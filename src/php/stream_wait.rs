//! Waiting on a PHP thread for room in a stream's chunk queue.
//!
//! A stream's flush hands its output to a bounded queue that the connection
//! drains as the client reads. When the queue is full the flush has to wait,
//! and it does so one of two ways. Where its request can be parked, it is, so
//! that the worker runs its other requests meanwhile: `StreamSender::try_room`
//! arms a wait for the next free slot and hands back a descriptor that turns
//! readable once that wait ends, for the worker's scheduler to resume the
//! request on — at once, as it resumes a request waiting on a socket. Where
//! the request cannot be parked, `StreamSender::wait_here` blocks the thread
//! instead. A wait that only ends when room appears ends never for a client
//! that has stopped reading — while the request it holds may meanwhile have
//! been told to end. So the blocking wait also stops when its caller says to,
//! which it is asked at least every `interval`; a parked request's caller sets
//! a time limit on the park for the same reason.
//!
//! Polled by hand rather than through a runtime: `Sender::reserve` needs no
//! runtime to make progress (it is what `blocking_send` waits on), and a wait
//! with a runtime timer in it panics if the runtime has shut down first — on
//! a PHP thread, inside a SAPI callback, which is an abort.

use std::future::Future;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;
use std::time::Duration;

use tokio::sync::mpsc::error::{SendError, TrySendError};
use tokio::sync::mpsc::{OwnedPermit, Permit, Sender};

/// Why there is no room for a message.
#[derive(Debug, PartialEq, Eq)]
pub enum NoRoom {
    /// The receiver is gone: the client has left.
    Closed,
    /// The caller's `stop` answered true.
    Stopped,
    /// None yet, and a wait for it is armed. Carries the descriptor that turns
    /// readable once the wait ends — or `None` when no descriptor could be
    /// made, and the caller has to look again on a timer of its own.
    Full(Option<RawFd>),
}

/// Room in the queue for one message, taken, so the send that follows cannot
/// find it gone.
pub enum Room<'a, T> {
    /// Free when asked for.
    Free(Permit<'a, T>),
    /// Waited for.
    Reserved(OwnedPermit<T>),
}

impl<T> Room<'_, T> {
    pub fn send(self, value: T) {
        match self {
            Room::Free(permit) => permit.send(value),
            Room::Reserved(permit) => drop(permit.send(value)),
        }
    }
}

/// What a parked flush waits on: one end of a socket pair, which turns
/// readable when the wait for room it belongs to is woken and the wake writes
/// to the other end. A descriptor rather than a flag, because the scheduler a
/// request is parked with waits on descriptors, and wakes for one at once.
struct RoomSignal {
    read: UnixStream,
    write: UnixStream,
}

impl RoomSignal {
    fn new() -> std::io::Result<Self> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        Ok(Self { read, write })
    }

    /// Take what earlier wakes wrote, so that the descriptor turns readable
    /// again only for a wake made after this.
    fn clear(&self) {
        let mut buf = [0u8; 64];
        while matches!((&self.read).read(&mut buf), Ok(n) if n > 0) {}
    }
}

impl Wake for RoomSignal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // A write that finds no space finds the descriptor readable already.
        let _ = (&self.write).write(&[1]);
    }
}

type Reserve<T> = Pin<Box<dyn Future<Output = Result<OwnedPermit<T>, SendError<()>>> + Send>>;

/// A wait for room, armed by a flush that found none.
struct Armed<T> {
    reserve: Reserve<T>,
    signal: Arc<RoomSignal>,
}

/// A stream's chunk sender, and the wait for room its last flush left armed.
/// The two are kept together so that the wait cannot outlive the stream: it
/// holds a sender of its own, as does the room it is granted until that is
/// used, and the stream's receiver sees it end only once every sender of its
/// queue is gone.
pub struct StreamSender<T> {
    tx: Sender<T>,
    /// Made the first time the queue is found full and kept for the rest of
    /// the stream, so a client that keeps the queue full costs no new
    /// descriptors on each wait. `None` until then, and while one cannot be
    /// made.
    signal: Option<Arc<RoomSignal>>,
    /// While a wait is armed, room is looked for through it rather than
    /// through `tx`: the next slot to free goes to it first.
    armed: Option<Armed<T>>,
}

impl<T: Send + 'static> StreamSender<T> {
    pub fn new(tx: Sender<T>) -> Self {
        Self {
            tx,
            signal: None,
            armed: None,
        }
    }

    pub fn sender(&self) -> &Sender<T> {
        &self.tx
    }

    /// The sender alone, for a last send made elsewhere. A wait armed goes.
    pub fn into_sender(self) -> Sender<T> {
        self.tx
    }

    /// Room for one message if there is some now. Otherwise a wait for it is
    /// armed, or kept if one already is, and `NoRoom::Full` names the
    /// descriptor that turns readable once the wait ends: with room, or with
    /// the receiver gone, which is what the next call then returns. A call
    /// that finds the wait still going — the caller looked early — returns the
    /// same descriptor again.
    pub fn try_room(&mut self) -> Result<Room<'_, T>, NoRoom> {
        let mut armed = match self.armed.take() {
            Some(armed) => armed,
            None => {
                match self.tx.try_reserve() {
                    Ok(permit) => return Ok(Room::Free(permit)),
                    Err(TrySendError::Closed(())) => return Err(NoRoom::Closed),
                    Err(TrySendError::Full(())) => {}
                }
                if self.signal.is_none() {
                    self.signal = RoomSignal::new().ok().map(Arc::new);
                }
                let Some(signal) = &self.signal else {
                    return Err(NoRoom::Full(None));
                };
                Armed {
                    reserve: Box::pin(self.tx.clone().reserve_owned()),
                    signal: Arc::clone(signal),
                }
            }
        };
        // Emptied before the poll and never after it: the poll is what
        // registers the wake, so whatever a wake writes from then on is still
        // there for the caller's wait to find.
        armed.signal.clear();
        let waker = Waker::from(Arc::clone(&armed.signal));
        match armed
            .reserve
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
        {
            Poll::Ready(Ok(permit)) => Ok(Room::Reserved(permit)),
            Poll::Ready(Err(_)) => Err(NoRoom::Closed),
            Poll::Pending => {
                let fd = armed.signal.read.as_raw_fd();
                self.armed = Some(armed);
                Err(NoRoom::Full(Some(fd)))
            }
        }
    }

    /// Block the calling thread for room, as `wait_for_room` describes. A wait
    /// `try_room` armed goes first: it is ahead in line for the next slot to
    /// free, which would go to a wake nobody is waiting for.
    pub fn wait_here(
        &mut self,
        interval: Duration,
        stop: impl FnMut() -> bool,
    ) -> Result<Room<'_, T>, NoRoom> {
        self.armed = None;
        wait_for_room(&self.tx, interval, stop).map(Room::Free)
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Block the calling thread until `tx` has room for one more message, its
/// receiver is gone, or `stop` answers true. `stop` is asked after every wake
/// that brought no room, and no more than `interval` apart; room that appears
/// wakes the thread at once.
///
/// On room, returns the permit for the slot, so the send that follows cannot
/// find it taken.
fn wait_for_room<T>(
    tx: &Sender<T>,
    interval: Duration,
    mut stop: impl FnMut() -> bool,
) -> Result<Permit<'_, T>, NoRoom> {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut reserve = pin!(tx.reserve());
    loop {
        match reserve.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(permit)) => return Ok(permit),
            Poll::Ready(Err(_)) => return Err(NoRoom::Closed),
            Poll::Pending => {}
        }
        if stop() {
            return Err(NoRoom::Stopped);
        }
        std::thread::park_timeout(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn full_channel() -> (Sender<u32>, tokio::sync::mpsc::Receiver<u32>) {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.try_send(0).unwrap();
        (tx, rx)
    }

    #[test]
    fn room_already_there_is_taken_without_asking_stop() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<u32>(1);
        let permit = wait_for_room(&tx, Duration::from_secs(5), || panic!("asked to stop"));
        permit.unwrap().send(1);
        assert_eq!(tx.capacity(), 0);
    }

    #[test]
    fn room_made_by_the_reader_wakes_the_wait_at_once() {
        let (tx, mut rx) = full_channel();
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let first = rx.blocking_recv();
            (first, rx.blocking_recv())
        });

        // An interval far longer than the test: only the wake can end the wait.
        let started = Instant::now();
        let permit = wait_for_room(&tx, Duration::from_secs(30), || false).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        permit.send(7);

        let (first, second) = reader.join().unwrap();
        assert_eq!(first, Some(0));
        assert_eq!(second, Some(7));
    }

    #[test]
    fn a_reader_that_leaves_ends_the_wait_as_closed() {
        let (tx, rx) = full_channel();
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(rx);
        });

        let end = wait_for_room(&tx, Duration::from_secs(30), || false).unwrap_err();
        assert_eq!(end, NoRoom::Closed);
        reader.join().unwrap();
    }

    #[test]
    fn stop_is_asked_again_while_nothing_moves() {
        let (tx, _rx) = full_channel();
        let asked = AtomicUsize::new(0);

        // Nothing wakes this wait but its interval, so each question after the
        // first is one the interval brought.
        let end = wait_for_room(&tx, Duration::from_millis(5), || {
            asked.fetch_add(1, Ordering::Relaxed) == 3
        })
        .unwrap_err();

        assert_eq!(end, NoRoom::Stopped);
        assert_eq!(asked.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn a_wait_that_stopped_leaves_the_queue_as_it_was() {
        let (tx, mut rx) = full_channel();
        assert_eq!(
            wait_for_room(&tx, Duration::from_millis(1), || true).unwrap_err(),
            NoRoom::Stopped
        );
        // Its place in the line for the next slot went with it.
        assert_eq!(rx.try_recv(), Ok(0));
        tx.try_send(1).unwrap();
    }

    /// Whether `fd` turns readable within `ms`.
    fn readable(fd: RawFd, ms: i32) -> bool {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut pfd, 1, ms) == 1 }
    }

    fn full_stream() -> (StreamSender<u32>, tokio::sync::mpsc::Receiver<u32>) {
        let (tx, rx) = full_channel();
        (StreamSender::new(tx), rx)
    }

    fn armed_fd(stream: &mut StreamSender<u32>) -> RawFd {
        match stream.try_room() {
            Err(NoRoom::Full(Some(fd))) => fd,
            Err(other) => panic!("expected an armed wait, got {other:?}"),
            Ok(_) => panic!("expected an armed wait, got room"),
        }
    }

    #[test]
    fn room_found_free_is_taken_without_arming_a_wait() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(1);
        let mut stream = StreamSender::new(tx);
        stream.try_room().ok().unwrap().send(3);
        assert!(stream.armed.is_none() && stream.signal.is_none());
        assert_eq!(rx.try_recv(), Ok(3));
    }

    #[test]
    fn the_descriptor_of_an_armed_wait_turns_readable_when_the_reader_takes_one() {
        let (mut stream, mut rx) = full_stream();
        let fd = armed_fd(&mut stream);
        assert!(!readable(fd, 0));

        assert_eq!(rx.try_recv(), Ok(0));
        assert!(readable(fd, 5000));
        stream.try_room().ok().unwrap().send(7);
        assert_eq!(rx.try_recv(), Ok(7));
    }

    #[test]
    fn an_armed_wait_ends_as_closed_when_the_reader_leaves() {
        let (mut stream, rx) = full_stream();
        let fd = armed_fd(&mut stream);
        drop(rx);
        assert!(readable(fd, 5000));
        assert_eq!(stream.try_room().err(), Some(NoRoom::Closed));
    }

    #[test]
    fn looking_early_keeps_the_wait_and_its_descriptor() {
        let (mut stream, mut rx) = full_stream();
        let fd = armed_fd(&mut stream);
        assert_eq!(armed_fd(&mut stream), fd);
        assert!(!readable(fd, 0));

        assert_eq!(rx.try_recv(), Ok(0));
        assert!(readable(fd, 5000));
    }

    #[test]
    fn a_wake_already_consumed_does_not_make_the_next_wait_readable() {
        let (mut stream, mut rx) = full_stream();
        let fd = armed_fd(&mut stream);
        assert_eq!(rx.try_recv(), Ok(0));
        assert!(readable(fd, 5000));
        stream.try_room().ok().unwrap().send(1);

        // Full again: the next wait reuses the descriptor, which says nothing
        // until this wait is woken.
        assert_eq!(armed_fd(&mut stream), fd);
        assert!(!readable(fd, 0));
    }

    #[test]
    fn a_stream_ends_with_its_armed_wait() {
        let (mut stream, mut rx) = full_stream();
        armed_fd(&mut stream);
        drop(stream);
        assert_eq!(rx.try_recv(), Ok(0));
        assert_eq!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        );
    }

    #[test]
    fn waiting_here_drops_the_wait_armed_ahead_of_it() {
        let (mut stream, mut rx) = full_stream();
        armed_fd(&mut stream);
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let first = rx.blocking_recv();
            (first, rx.blocking_recv())
        });

        // The reader frees one slot only. Left armed, the wait above would
        // take it, and this one would wait until it gave up.
        let started = Instant::now();
        let room = stream.wait_here(Duration::from_millis(20), || {
            started.elapsed() > Duration::from_secs(5)
        });
        room.ok().unwrap().send(9);

        let (first, second) = reader.join().unwrap();
        assert_eq!(first, Some(0));
        assert_eq!(second, Some(9));
    }
}

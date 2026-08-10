<?php

declare(strict_types=1);

// Inner self-request for the two hooks/test_parse_* tests below. It is served
// only while the outer request's fiber is parked in a hooked sleep(), so its
// body is parsed on the event-loop dispatch path rather than the fast one —
// the other of the two places a worker-mode request's input is built.
//
// The body it is sent is deliberately over max_file_uploads, which is what
// makes its parse raise into whatever error handler the outer request left
// installed. Everything interesting therefore happens before this file runs;
// all it has to do is say that it ran, and with what.
//
// No TestCase here on purpose: that installs an error handler of its own, and
// the handler under test is the one the outer request installed.
//
// Must not suspend: no await, no sleep, no socket read. A suspend here would
// park this fiber too, and the outer request could resume before this one had
// been parsed at all.

echo 'INNER-OK files=' . count($_FILES) . ' post=' . count($_POST) . "\n";

<?php

declare(strict_types=1);

// A resume gives back the session state standing on the thread after it has
// freed what a finished request left there, because a destructor that runs as
// that is freed can open one. Giving it back runs PHP too, and from PHP 8.5 an
// E_USER_ERROR raised in it leaves a backtrace on the thread even when an error
// handler takes the error over. That backtrace is nobody's and is let go of
// before the resumed request is put back, so what its frames were given is
// released rather than kept until the worker exits — and the session state a
// destructor run by letting go of it opens is given back before the resumed
// request's own session is put back.
//
// A parked request waits 800 ms. A leaver sent meanwhile leaves an output
// buffer whose destructor fills $_SESSION, and ends. The parked request is the
// next to resume. Before 8.5 there is no backtrace: the argument goes as the
// call that raised the error returns, and leaves no session behind.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$sharedState['cell_session_bt_parked'] = false;
$sharedState['cell_session_bt_left'] = false;
$known = class_exists('ThreadCellSessionBacktraceLeaver', false);
$leaverRunsBefore = $known ? ThreadCellSessionBacktraceLeaver::$runs : 0;
$bombRunsBefore = $known ? ThreadCellSessionBacktraceBomb::$runs : 0;
$releasedBefore = $known ? ThreadCellSessionBacktraceArgument::$released : 0;
if ($known) {
    ThreadCellSessionBacktraceBomb::$handled = false;
    ThreadCellSessionBacktraceBomb::$heldAfterTheCall = null;
    ThreadCellSessionBacktraceArgument::$leftSession = false;
}
$parkedSock = thread_cell_send('/tests/fibers/fixture_restore_session_backtrace.php?role=parked&who=parked');
$parked = thread_cell_wait_for($sharedState, 'cell_session_bt_parked');
$leaverSock = thread_cell_send('/tests/fibers/fixture_restore_session_backtrace.php?role=leaver&who=leaver');
// The parked request's answer first: this request must not resume — which
// would free what the leaver left — before the parked one does.
$resumed = thread_cell_read($parkedSock);
$leaver = thread_cell_read($leaverSock);
$leaverRuns = ThreadCellSessionBacktraceLeaver::$runs - $leaverRunsBefore;
$bombRuns = ThreadCellSessionBacktraceBomb::$runs - $bombRunsBefore;
$released = ThreadCellSessionBacktraceArgument::$released - $releasedBefore;

$t = new TestCase('resume_lets_go_of_a_backtrace_its_session_release_leaves', 'fibers');
$t->meta('resumed', $resumed);
$t->meta('leaver', $leaver);
$t->meta('leaver_destructor_runs', $leaverRuns);
$t->meta('session_destructor_runs', $bombRuns);
$t->meta('handled', ThreadCellSessionBacktraceBomb::$handled);
$t->meta('held_after_the_call', ThreadCellSessionBacktraceBomb::$heldAfterTheCall);
$t->meta('argument_released', $released);
$t->meta('argument_left_session', ThreadCellSessionBacktraceArgument::$leftSession);

$t->assertTrue('the request parked', $parked);
$t->assertTrue('the leaver ran and left its buffer while it was parked', $sharedState['cell_session_bt_left']);
$t->assertContains('the parked request resumed and answered', $resumed['body'], 'BODY:parked;');
$t->assertSame("the leaver's buffer was freed: its destructor filled \$_SESSION", $leaverRuns, 1);
$t->assertSame('and giving that session state back ran the destructor of what it held', $bombRuns, 1);
$t->assertTrue('which raised an E_USER_ERROR its error handler took over', ThreadCellSessionBacktraceBomb::$handled);
$t->assertSame(
    PHP_VERSION_ID >= 80500
        ? 'the backtrace of that error held the argument past the call'
        : 'with no backtrace, the argument went as the call returned',
    ThreadCellSessionBacktraceBomb::$heldAfterTheCall,
    PHP_VERSION_ID >= 80500
);
$t->assertSame('and the argument was released, once, before the parked request answered', $released, 1);
$t->assertSame(
    PHP_VERSION_ID >= 80500
        ? 'released with the backtrace, it filled $_SESSION again'
        : 'released as the call returned, it left no session',
    ThreadCellSessionBacktraceArgument::$leftSession,
    PHP_VERSION_ID >= 80500
);
$t->assertContains('and the parked request was put back with no $_SESSION standing', $resumed['body'], 'SESSION:none;');

$t->done();

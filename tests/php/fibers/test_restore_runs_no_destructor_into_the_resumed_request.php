<?php

declare(strict_types=1);

// Resuming a request runs no user code of what a finished request left on the
// thread while the resumed request's state is in place. What a finished request
// leaves standing — here an output buffer its end could not remove — is freed
// before the resumed request is put back, so a destructor that runs as it is
// freed writes into no request's response, sends no request's headers, reads
// no request's $_GET, and registers its shutdown function on no request.
//
// A parked request waits 800 ms. A leaver sent meanwhile leaves the buffer and
// ends. The parked request is the next to resume, and sets a header once it
// has. The buffer is still freed: the destructor runs, once, and the shutdown
// function it registered is let go of without running.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

$sharedState['cell_restore_parked'] = false;
$sharedState['cell_restore_left'] = false;
$known = class_exists('ThreadCellRestoreDestructor', false);
$runsBefore = $known ? ThreadCellRestoreDestructor::$runs : 0;
$lateBefore = $known ? ThreadCellRestoreDestructor::$lateReleased : 0;
$parkedSock = thread_cell_send('/tests/fibers/fixture_restore_destructor.php?role=parked&who=parked');
$parked = thread_cell_wait_for($sharedState, 'cell_restore_parked');
$leaverSock = thread_cell_send('/tests/fibers/fixture_restore_destructor.php?role=leaver&who=leaver');
// The parked request's answer first: this request must not resume — which
// would free what the leaver left — before the parked one does.
$resumed = thread_cell_read($parkedSock);
$leaver = thread_cell_read($leaverSock);
$runs = ThreadCellRestoreDestructor::$runs - $runsBefore;
$lateReleased = ThreadCellRestoreDestructor::$lateReleased - $lateBefore;

$t = new TestCase('restore_runs_no_destructor_into_the_resumed_request', 'fibers');
$t->meta('resumed', $resumed);
$t->meta('leaver', $leaver);
$t->meta('destructor_runs', $runs);
$t->meta('late_shutdown_released', $lateReleased);

$t->assertTrue('the request parked', $parked);
$t->assertTrue('the leaver ran and left its buffer while it was parked', $sharedState['cell_restore_left']);
$t->assertNotContains("the leaver's destructor wrote nothing into the resumed request", $resumed['body'], 'DTOR:');
$t->assertContains('the resumed request set its header after the resume', strtolower($resumed['headers']), 'x-after-resume: 1');
$t->assertContains('the resumed request wrote its own body', $resumed['body'], 'BODY:parked;');
$t->assertContains('and ran its own shutdown function', $resumed['body'], 'OWN-SHUTDOWN:parked;');
$t->assertNotContains("and not the one the leaver's destructor registered", $resumed['body'], 'LATE-SHUTDOWN;');
$t->assertSame('what the leaver left was let go of: its destructor ran once', $runs, 1);
$t->assertSame('and the shutdown function it registered was let go of', $lateReleased, 1);

$t->done();

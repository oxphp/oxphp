<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() runs the error handler before it buffers anything when
// an option is a quantity it cannot parse, and a body the handler makes there
// is put out of SG() by the call's own parse a moment later. It is neither what
// stood when the call was made nor what the call made, so it has to be kept for
// the end of the request from the moment it is replaced.
//
// case=nested: a POST, whose body the request parsed before the script; the
// handler calls request_parse_body() again, which buffers a body of its own.
// case=input: a PUT, which has no body until something asks for one; the
// handler opens php://input, which makes the body, and reads it through, as an
// error reporter attaching the request body would. The handle it keeps must be
// closed with its request, as one opened anywhere else is.
//
// Three sightings of each on one worker (PHP_WORKERS=1 in this profile); the
// count assertion compares the second and third, since the first also pays for
// whatever the request path allocates once.

$t = new TestCase('request_parse_body_options_warning_streams_bounded', 'hooks');

$case = (string) ($_GET['case'] ?? '');
$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertTrue('the suite line names the case', in_array($case, ['nested', 'input'], true));
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);
$key = 'request_parse_body_options_warning_' . $case;

if ($case === 'input' && $sighting > 1) {
    $t->assertKeyExists(
        'the request before this one left the handler\'s handle in worker-scope state',
        $sharedState,
        $key . '_handle'
    );
    $t->assertFalse(
        'the handle the error handler opened is closed with its request',
        is_resource($sharedState[$key . '_handle'] ?? null)
    );
    unset($sharedState[$key . '_handle']);
}

$entered = false;
$madeInHandler = null;
$readInHandler = null;
$handle = null;
set_error_handler(static function () use ($case, &$entered, &$madeInHandler, &$readInHandler, &$handle): bool {
    if (!$entered) {
        $entered = true;
        $before = count(get_resources('stream'));
        if ($case === 'nested') {
            $readInHandler = request_parse_body();
        } else {
            $handle = fopen('php://input', 'r');
            $readInHandler = stream_get_contents($handle);
        }
        $madeInHandler = count(get_resources('stream')) - $before;
    }
    return true;
});
$before = count(get_resources('stream'));
try {
    request_parse_body(['max_input_vars' => '1000Q']);
} finally {
    restore_error_handler();
}
$madeByCall = count(get_resources('stream')) - $before;

// The premise: the handler ran inside the call and made a body there, and the
// call went on to buffer one of its own over it. Without both, nothing made in
// the handler is replaced and the counts below stay flat on any build.
$t->meta('streams_made_in_the_handler', $madeInHandler);
$t->meta('streams_made_by_the_call', $madeByCall);
if ($case === 'nested') {
    $t->assertSame('the inner call found the body already read', $readInHandler, [[], []]);
} else {
    $t->assertSame('the handler read the body through its own handle', $readInHandler, 'probe=window');
}
$t->assertGreaterThan('the handler made a body', (int) $madeInHandler, 0);
$t->assertGreaterThan('and the call buffered another', $madeByCall, (int) $madeInHandler);

$counts = $sighting > 1 ? (array) ($sharedState[$key . '_counts'] ?? []) : [];
$counts[] = count(get_resources('stream'));
$sharedState[$key . '_counts'] = $counts;
$t->meta('stream_counts', $counts);

if ($sighting < 3) {
    if ($handle !== null) {
        $sharedState[$key . '_handle'] = $handle;
    }
    $t->assertTrue('collecting stream counts (sighting ' . $sighting . ' of 3)', true);
} else {
    if ($handle !== null) {
        fclose($handle);
    }
    unset($sharedState[$key . '_counts']);
    $t->assertCount('all three sightings landed on this worker', $counts, 3);
    $t->assertSame(
        'open stream count does not grow from one such request to the next',
        $counts[2] ?? null,
        $counts[1] ?? null
    );
}

$t->done();

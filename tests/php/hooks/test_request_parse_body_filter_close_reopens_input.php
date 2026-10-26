<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request_parse_body() call that exits before replacing the body leaves it
// current, and the end of the request closes it once. Closing it calls
// onclose() on every userland filter on it, and one that opens php://input
// there is handed whatever body the request still names. The close must have
// stopped naming the body first: handed the stream being closed, the opener
// rewinds a stream with nothing left behind it, which takes the worker down.
//
// Two sightings on one worker (PHP_WORKERS=1 in this profile):
//   1. a filter goes on the body, and a call over post_max_size leaves it current;
//   2. what the filter's php://input read at the end of sighting 1.

$t = new TestCase('request_parse_body_filter_close_reopens_input', 'hooks');

$sighting = (int) ($_GET['sighting'] ?? 0);
$t->assertGreaterThan('the suite line names this sighting', $sighting, 0);

$marker = '/tmp/oxphp-request-parse-body-filter-close-reopen';

if (!class_exists('OxPHPReopenInputOnCloseFilter', false)) {
    class OxPHPReopenInputOnCloseFilter extends php_user_filter
    {
        public function onClose(): void
        {
            $input = fopen('php://input', 'r');
            file_put_contents(
                '/tmp/oxphp-request-parse-body-filter-close-reopen',
                json_encode(['read' => $input === false ? null : stream_get_contents($input)])
            );
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

if (!in_array('oxphp-reopen-input-on-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-reopen-input-on-close', OxPHPReopenInputOnCloseFilter::class);
}

if ($sighting === 1) {
    if (is_file($marker)) {
        unlink($marker);
    }

    // The body is a temp stream with no URI, and the newest one: resource ids
    // only grow inside a worker, and the script has opened nothing yet.
    $bodyId = null;
    foreach (get_resources('stream') as $id => $res) {
        if (stream_get_meta_data($res)['stream_type'] === 'TEMP'
            && !isset(stream_get_meta_data($res)['uri'])
            && ($bodyId === null || $id > $bodyId)) {
            $bodyId = $id;
        }
    }
    $t->assertNotNull('the request body is reachable as a stream resource', $bodyId);

    if ($bodyId !== null) {
        stream_filter_append(get_resources('stream')[$bodyId], 'oxphp-reopen-input-on-close', STREAM_FILTER_READ);
    }

    $before = count(get_resources('stream'));
    set_error_handler(static fn (): bool => true);
    try {
        request_parse_body(['post_max_size' => 1]);
    } finally {
        restore_error_handler();
    }
    // The premise: over post_max_size the parse buffers no body of its own, so
    // the body the filter is on is still the request's when the request ends.
    $t->assertSame(
        'the call left the body standing',
        count(get_resources('stream')) - $before,
        0
    );
} else {
    $t->assertTrue('the end of the request before closed the filtered body', is_file($marker));
    $seen = is_file($marker) ? json_decode((string) file_get_contents($marker), true) : null;
    if (is_file($marker)) {
        unlink($marker);
    }
    $t->meta('seen', $seen);
    $t->assertSame(
        'the php://input its filter opened there found no body standing',
        $seen,
        ['read' => '']
    );
    $t->assertGreaterThan(
        'the worker that ran the close is still serving',
        OxPHP\Server\Worker::current()->requestCount(),
        1
    );
}

$t->done();

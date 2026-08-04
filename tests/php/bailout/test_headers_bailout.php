<?php

declare(strict_types=1);

// `OxPHP\Http\Request::headers()` walks the per-request state the server holds
// and hands each header to PHP one at a time, so it allocates on the Zend heap
// in the middle of reading that state. An allocation that hits memory_limit
// raises a fatal, and a PHP fatal unwinds by longjmp: it skips every
// destructor between the failed allocation and PHP's own handler. If the
// server is still holding a read of its request state when that happens, the
// read is never given back, and the next access to that state takes down the
// whole process instead of failing this one request — PHP reads the request
// body through that same state from request shutdown, on every request.
//
// The trap: fill the memory the allocator has mapped, forbid it from mapping
// more, and then ask for the headers. One of them (sent by the runner, see
// suites/bailout.txt) is far larger than the tail left over, so copying it is
// the allocation that cannot be served.
//
// This request is expected to answer 500 — the fatal is real. What it proves
// is checked by test_worker_survives_bailout.php, which runs next and reads
// the marker written below: on a server carrying the defect that request is
// answered by nobody, because the process is gone.
//
// No TestCase here — it installs a fatal handler of its own, and this test has
// to inspect the fatal itself.

const MARKER_PATH = '/tmp/oxphp-bailout-marker.json';

/** A block of this size costs two pages, so it is never served from a bin. */
const BLOCK_BYTES = 4096;
const BLOCK_COST  = 2 * 4096;

/** Free tail to leave behind: smaller than the padding header, larger than 0. */
const TAIL_BYTES = 96 * 1024;

/** Size of the `X-Pad` header the runner sends — see suites/bailout.txt. */
const PAD_HEADER_BYTES = 200000;

@unlink(MARKER_PATH);

$request = oxphp_http_request();
$trapLine = 0;

register_shutdown_function(static function () use (&$trapLine): void {
    // The trap leaves the heap with no headroom at all, and writing the marker
    // needs some back.
    ini_set('memory_limit', '64M');

    file_put_contents(MARKER_PATH, json_encode([
        'expected_line'  => $trapLine,
        'expected_bytes' => PAD_HEADER_BYTES,
        'error'          => error_get_last(),
    ]));
});

// Fill everything the allocator has already mapped, stopping as soon as it
// takes a fresh chunk from the operating system: at that instant every earlier
// chunk is full, and the size of the jump is the chunk size.
$ballast = [];
$blocks  = 0;
$mapped  = memory_get_usage(true);
while (memory_get_usage(true) === $mapped && $blocks < 4000) {
    // The length has to vary: with a constant one the compiler folds the call
    // to a literal, every element ends up sharing it, and nothing is allocated.
    $ballast[] = str_repeat('x', BLOCK_BYTES + (++$blocks % 8));
}
$chunkBytes = memory_get_usage(true) - $mapped;

// Fill the fresh chunk as well, stopping short by a tail the padding header
// cannot fit in.
$fill = intdiv($chunkBytes - TAIL_BYTES, BLOCK_COST);
for ($i = 0; $i < $fill; $i++) {
    $ballast[] = str_repeat('x', BLOCK_BYTES + ($i % 8));
}

// Now forbid a further chunk. The first call drops chunks the allocator has
// cached — it reports success without applying the limit — and the second one
// applies the limit to what is actually mapped. The failure of the first is
// expected and would otherwise stand as the request's last error.
@ini_set('memory_limit', (string) (memory_get_usage(true) - 1));
ini_set('memory_limit', (string) memory_get_usage(true));
error_clear_last();

// Assigning an integer over an existing variable allocates nothing, so this
// cannot disturb the headroom the next line depends on.
$trapLine = __LINE__ + 1;
$headers = $request->headers();

echo "FAIL: headers() returned ", count($headers), " entries — the trap never armed\n";

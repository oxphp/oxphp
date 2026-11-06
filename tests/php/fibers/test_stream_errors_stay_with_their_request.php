<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// PHP 8.6 keeps stream errors on the thread: the stack of operations stream
// functions have open, which an error raised anywhere joins and which reports it
// as it ends, and what stream_last_errors() answers with. A worker runs every
// request it serves on one thread, so all of that belongs to the request that is
// running. From a request that parks while others run:
//
//   - a request served in the window does not read this one's stored errors, nor
//     those of the request served just before it, and leaves none of its own;
//   - an error raised in the window by something with no operation of its own, a
//     filter writing out what it holds as its stream is released, goes to the
//     request that raised it, not into the stream_get_contents() this one is
//     parked in.
//
// Before 8.6 there is nothing stored, and the second half holds anyway.

// Cleared so the only handler this request hears from is the one it sets below.
set_error_handler(null);
set_exception_handler(null);

$t = new TestCase('stream_errors_stay_with_their_request', 'fibers');

$stored = function_exists('stream_last_errors');
$messages = static fn (): array => array_map(static fn ($e): string => $e->message, stream_last_errors());
$own = [];
if ($stored) {
    fopen('/nonexistent/oxphp-outer', 'r', false, stream_context_create(['stream' => ['error_mode' => StreamErrorMode::Silent]]));
    $own = $messages();
    $t->assertContains('this request stores an error of its own', $own[0] ?? '', 'No such file or directory');
}

$send = static function (string $query) use ($t) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("inner self-request socket connected ($query)", $sock !== false);
    if ($sock === false) {
        $t->done();
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_stream_errors_of_a_neighbour.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    return $sock;
};

// Both inside one park, so the second starts with nothing having run in between
// to clear what the first left. Whichever is served second must read none of it.
$first = $send('store=1');
$second = $send('store=1');
sleep(1); // Hooked: parks this request's fiber. PHP_WORKERS=1 in this profile.
$firstResponse = (string) stream_get_contents($first);
$secondResponse = (string) stream_get_contents($second);
fclose($first);
fclose($second);
$t->assertContains('a request served in the window reads no stored error', $firstResponse, 'STORED-AT-START:[]');
$t->assertContains('nor does the one served after it', $secondResponse, 'STORED-AT-START:[]');
if ($stored) {
    $t->assertSame('this request reads only its own after resuming', $messages(), $own);
}

$heard = [];
set_error_handler(static function (int $severity, string $message) use (&$heard): bool {
    $heard[] = $message;

    return true;
});
$closing = $send('close=1');
$closingResponse = (string) stream_get_contents($closing);
fclose($closing);
restore_error_handler();
$t->meta('neighbour', $closingResponse);
$t->assertContains('the neighbour hears of the error its stream raised', $closingResponse, 'Send of ');
$t->assertSame('and the operation parked here takes none of it', $heard, []);

$t->done();

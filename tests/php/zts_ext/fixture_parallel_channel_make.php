<?php

declare(strict_types=1);

// Driven by test_parallel_channel_outlives_request as a second HTTP request, so
// that whatever it reports about a channel name is reported from outside the
// request that created it.
//
// Answers two things about the name it is given, in one line of plain text:
//   MAKE=<class|none>  what Channel::make() on an existing name does here
//   SEND=<ok|error>    whether Channel::open() reaches the same channel object
//                      the other request is holding

$name = (string) ($_GET['name'] ?? '');
$payload = (string) ($_GET['payload'] ?? '');

$make = 'none';
try {
    \parallel\Channel::make($name, \parallel\Channel::Infinite);
} catch (\Throwable $e) {
    $make = get_class($e);
}

$send = 'ok';
try {
    \parallel\Channel::open($name)->send($payload);
} catch (\Throwable $e) {
    $send = 'error:' . $e->getMessage();
}

header('Content-Type: text/plain');
echo 'MAKE=', $make, ';SEND=', $send;

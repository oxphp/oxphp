<?php
/** Exhaustive match on RecvStatus. */
use OxPHP\Shared\Channel;
use OxPHP\Shared\Channel\RecvStatus;

header('Content-Type: text/plain');

$ch = new Channel(2);
$ch->send('hi');

$dispatch = function ($r): string {
    return match ($r->status()) {
        RecvStatus::Ok      => "ok:" . $r->value(),
        RecvStatus::Empty   => "empty",
        RecvStatus::Timeout => "timeout",
        RecvStatus::Closed  => "closed",
    };
};

if ($dispatch($ch->tryRecv()) !== "ok:hi")   { echo "FAIL: ok\n"; exit; }
if ($dispatch($ch->tryRecv()) !== "empty")   { echo "FAIL: empty\n"; exit; }
if ($dispatch($ch->recvTimeout(20)) !== "timeout") { echo "FAIL: timeout\n"; exit; }
$ch->close();
if ($dispatch($ch->tryRecv()) !== "closed")  { echo "FAIL: closed\n"; exit; }

echo "OK\n";

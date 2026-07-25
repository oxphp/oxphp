<?php

/**
 * Channel CSP demo — message passing between PHP threads.
 *
 * Every mode below uses only shipped API: OxPHP\Shared\Channel handles are
 * captured by oxphp_async() closures, so both sides address the same
 * process-global primitive rather than a copy.
 *
 * Query params:
 *   ?mode=fanin     — N producers → one channel → M consumers (default)
 *   ?mode=pipeline  — two-stage pipeline, each stage its own task
 *   ?mode=poll      — waiting on two channels at once, and what it costs
 *   ?n=N            — message count (fanin, pipeline), capped at 5000
 *
 * Requires ASYNC_WORKERS > 0 (the async pool is disabled by default).
 *
 * Sizing note: consumers use recvTimeout() rather than recv(), and channel
 * capacity covers the whole batch. That keeps every mode completable no
 * matter how ASYNC_WORKERS compares to the number of dispatched tasks —
 * outside worker mode these calls block their pool thread instead of
 * suspending a fiber.
 */

if (!function_exists('oxphp_async')) {
    json_response(503, [
        'error'  => 'Async not available',
        'detail' => 'oxphp_async() requires the OxPHP server with ASYNC_WORKERS enabled.',
    ]);
    return;
}

if (!class_exists('OxPHP\\Shared\\Channel')) {
    json_response(503, [
        'error'  => 'Shared state not available',
        'detail' => 'OxPHP\\Shared\\Channel requires the shared-state plugin (SHARED_ENABLED=true).',
    ]);
    return;
}

$mode = $_GET['mode'] ?? 'fanin';
$n    = min(max((int)($_GET['n'] ?? 200), 1), 5000);
$start = microtime(true);

switch ($mode) {
    case 'fanin':
        $producer_count = 2;
        $consumer_count = 3;
        $per            = max(intdiv($n, $producer_count), 1);
        $total          = $per * $producer_count;

        // Capacity covers the whole batch, so a producer never waits on a
        // full channel and the demo needs no ordering between the two roles.
        $ch = new OxPHP\Shared\Channel($total);

        $producers = [];
        for ($i = 0; $i < $producer_count; $i++) {
            $producers[] = oxphp_async(function () use ($ch, $per, $i): int {
                for ($j = 0; $j < $per; $j++) {
                    $ch->send(['producer' => $i, 'seq' => $j]);
                }
                return $per;
            });
        }

        $consumers = [];
        for ($i = 0; $i < $consumer_count; $i++) {
            $consumers[] = oxphp_async(function () use ($ch): int {
                $got = 0;
                while (true) {
                    $r = $ch->recvTimeout(250);
                    if (!$r->isOk()) {
                        break;              // Closed (drained) or Timeout
                    }
                    $got++;
                }
                return $got;
            });
        }

        $sent = array_sum(oxphp_async_await_all($producers));
        $ch->close();                        // idle consumers now see Closed
        $per_consumer = oxphp_async_await_all($consumers);
        $received     = array_sum($per_consumer);

        json_response(200, [
            'mode'         => 'fanin',
            'producers'    => $producer_count,
            'consumers'    => $consumer_count,
            'capacity'     => $total,
            'sent'         => $sent,
            'received'     => $received,
            'per_consumer' => array_values($per_consumer),
            'complete'     => $sent === $received,
            'wall_ms'      => round((microtime(true) - $start) * 1000, 1),
            'note'         => 'Work is distributed by the channel itself — no consumer is assigned a share up front.',
        ]);
        break;

    case 'pipeline':
        $ch1 = new OxPHP\Shared\Channel($n);
        $ch2 = new OxPHP\Shared\Channel($n);

        // Each stage closes its output channel when done, so the next stage
        // observes Closed instead of waiting out its timeout.
        $stage1 = oxphp_async(function () use ($ch1, $n): int {
            for ($i = 1; $i <= $n; $i++) {
                $ch1->send($i);
            }
            $ch1->close();
            return $n;
        });

        $stage2 = oxphp_async(function () use ($ch1, $ch2): int {
            $moved = 0;
            while (true) {
                $r = $ch1->recvTimeout(250);
                if (!$r->isOk()) {
                    break;
                }
                $ch2->send($r->value() ** 2);
                $moved++;
            }
            $ch2->close();
            return $moved;
        });

        $collector = oxphp_async(function () use ($ch2): array {
            $count = 0;
            $sum   = 0;
            while (true) {
                $r = $ch2->recvTimeout(250);
                if (!$r->isOk()) {
                    break;
                }
                $sum += $r->value();
                $count++;
            }
            return ['count' => $count, 'sum' => $sum];
        });

        $produced = oxphp_async_await($stage1);
        $moved    = oxphp_async_await($stage2);
        $out      = oxphp_async_await($collector);

        json_response(200, [
            'mode'      => 'pipeline',
            'stages'    => ['produce' => $produced, 'square' => $moved, 'collect' => $out['count']],
            'sum'       => $out['sum'],
            'expected'  => array_sum(array_map(static fn(int $i): int => $i ** 2, range(1, $n))),
            'wall_ms'   => round((microtime(true) - $start) * 1000, 1),
            'note'      => 'Stages run on separate threads; closing a channel is how a stage says "no more input".',
        ]);
        break;

    case 'poll':
        // Waiting on two channels at once has no single blocking call, so the
        // loop below polls: a non-blocking peek at the control channel plus a
        // bounded wait on the work channel. Both costs are measured.
        $quantum = min(max((int)($_GET['q'] ?? 50), 1), 500);
        $after   = min(max((int)($_GET['after'] ?? 150), 1), 2000);

        $work = new OxPHP\Shared\Channel(64);
        $ctrl = new OxPHP\Shared\Channel(1);

        $signal = oxphp_async(function () use ($ctrl, $after): float {
            usleep($after * 1000);
            $sent_at = microtime(true);
            $ctrl->send('stop');
            return $sent_at;
        });

        $idle_wakeups = 0;
        $detected_at  = null;

        while (true) {
            if ($ctrl->tryRecv()->isOk()) {
                $detected_at = microtime(true);
                break;
            }
            $w = $work->recvTimeout($quantum);
            if ($w->isTimeout()) {
                $idle_wakeups++;             // woke up, found nothing, looped
                continue;
            }
            if (!$w->isOk()) {
                break;                        // channel closed
            }
        }

        $sent_at = oxphp_async_await($signal);

        json_response(200, [
            'mode'          => 'poll',
            'quantum_ms'    => $quantum,
            'signalled_after_ms' => $after,
            'reaction_ms'   => round(($detected_at - $sent_at) * 1000, 1),
            'idle_wakeups'  => $idle_wakeups,
            'wall_ms'       => round((microtime(true) - $start) * 1000, 1),
            'note'          => 'Reaction latency is bounded by the poll quantum; idle wakeups grow as it shrinks. '
                             . 'Try ?q=5 and ?q=200 to see both ends of that trade.',
        ]);
        break;

    default:
        json_response(400, [
            'error' => 'Unknown mode',
            'valid' => ['fanin', 'pipeline', 'poll'],
        ]);
}

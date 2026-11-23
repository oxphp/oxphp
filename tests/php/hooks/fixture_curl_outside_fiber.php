<?php

declare(strict_types=1);

// Roles for test_curl_call_outside_a_fiber_is_refused_while_a_fiber_is_in_one:
// PHP that runs in no request, making cURL calls on the handles a request parked
// by fixture_curl_hold.php is in the middle of a call on.
//
// ?role=holder registers a userland save handler that nothing but the session
// module holds, opens its session and parks with it open for a second. The
// neighbour registers a handler of its own in that second, so once the holder's
// session is written and closed nothing holds the holder's handler any more, and
// its destructor runs between two slices of the worker, in no request. That
// destructor makes the calls ?via= names on $sharedState['curl_ch'] (and
// $sharedState['curl_mh']) and stores how each one ended in
// $sharedState['curl_outside']:
//
//   ?via=easy   the parked request is in curl_exec() on curl_ch: curl_setopt()
//               of CURLOPT_RETURNTRANSFER on it, and curl_multi_exec() given it.
//   ?via=multi  the parked request is in curl_multi_select() on curl_mh, curl_ch
//               in it: curl_setopt() of CURLOPT_URL on curl_ch, and
//               curl_multi_exec() on curl_mh.
//   ?via=member curl_setopt() of CURLOPT_URL on $sharedState['curl_member'], in a
//               multi handle no call is on, added by a request that is in
//               curl_exec() on another handle.

set_error_handler(null);
set_exception_handler(null);

$role = (string) ($_GET['role'] ?? '');
$via = (string) ($_GET['via'] ?? 'easy');

if (!class_exists('OxphpCurlOutsideFiberQuietHandler', false)) {
    class OxphpCurlOutsideFiberQuietHandler implements SessionHandlerInterface
    {
        public function open(string $path, string $name): bool
        {
            return true;
        }

        public function close(): bool
        {
            return true;
        }

        public function read(string $id): string
        {
            return '';
        }

        public function write(string $id, string $data): bool
        {
            return true;
        }

        public function destroy(string $id): bool
        {
            return true;
        }

        public function gc(int $max_lifetime): int|false
        {
            return 0;
        }

        // PHP 8.6 warns once for each of these a handler class lacks.
        public function create_sid(): string
        {
            return bin2hex(random_bytes(16));
        }

        public function validateId(string $id): bool
        {
            return true;
        }
    }

    final class OxphpCurlOutsideFiberHandler extends OxphpCurlOutsideFiberQuietHandler
    {
        public function __construct(private readonly \Closure $onDestruct)
        {
        }

        public function __destruct()
        {
            ($this->onDestruct)();
        }
    }
}

// PHP 8.6 turns session.use_strict_mode on by default; the ids here are the
// clients' cookies.
ini_set('session.use_strict_mode', '0');

header('Content-Type: application/json');

if ($role === 'holder') {
    $calls = static function () use (&$sharedState, $via): void {
        $attempt = static function (\Closure $call): string {
            try {
                return 'returned ' . var_export($call(), true);
            } catch (\Throwable $e) {
                return get_class($e) . ': ' . $e->getMessage();
            }
        };
        $out = [
            'request_id' => oxphp_request_id(),
            'curl_parked' => $sharedState['curl_parked'] ?? null,
            'in_select' => $sharedState['curl_in_select'] ?? null,
        ];
        $ch = $sharedState['curl_ch'];
        if ($via === 'member') {
            $out['adder_in_exec'] = $sharedState['curl_member_adder_in_exec'] ?? null;
            $member = $sharedState['curl_member'];
            $out['setopt'] = $attempt(static fn () => curl_setopt($member, CURLOPT_URL, 'http://127.0.0.1:1/member'));
        } elseif ($via === 'easy') {
            $out['setopt'] = $attempt(static fn () => curl_setopt($ch, CURLOPT_RETURNTRANSFER, false));
            $out['multi_exec'] = $attempt(static function () use ($ch): int {
                return curl_multi_exec($ch, $running);
            });
        } else {
            $mh = $sharedState['curl_mh'];
            $out['setopt'] = $attempt(static fn () => curl_setopt($ch, CURLOPT_URL, 'http://127.0.0.1:1/intruder'));
            $running = -1;
            $out['multi_exec'] = $attempt(static function () use ($mh, &$running): int {
                return curl_multi_exec($mh, $running);
            });
            $out['running'] = $running;
        }
        $sharedState['curl_outside'] = $out;
    };
    session_set_save_handler(new OxphpCurlOutsideFiberHandler($calls), false);
    session_start();
    $_SESSION['who'] = 'holder';
    oxphp_sleep(1.0);
    session_write_close();
    echo json_encode(['who' => 'holder']);
    return;
}

if ($role === 'neighbour') {
    $sharedState['curl_outside_neighbour'] = true;
    oxphp_sleep(0.4);
    $configured = session_set_save_handler(new OxphpCurlOutsideFiberQuietHandler(), false);
    echo json_encode(['who' => 'neighbour', 'configured' => $configured]);
    return;
}

http_response_code(400);
echo "unknown role: $role";

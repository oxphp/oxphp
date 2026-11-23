<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// `new PDO()` runs the driver's connect in a mode that skips the application's
// error handler and turns a warning into a PDOException, and puts the previous
// mode back once the connect returns. The mode is the thread's, and a worker runs
// every request it serves on one thread; with the socket hooks, a connect that
// waits on the server parks its request. So the mode has to go with it: a request
// served meanwhile raises its errors in its own mode, and the parked constructor
// raises its own in the mode it parked with.
//
// The holder connects to a listener on this worker's own 127.0.0.1:3306 that never
// accepts: the kernel completes the handshake from the backlog and mysqlnd waits
// for a greeting that does not come, until the listener is closed under it.
$t = new TestCase('error_mode_stays_with_a_request_parked_in_new_pdo', 'hooksdb');

$db = getenv('DB_NAME') ?: 'appdb';
$server = @stream_socket_server('tcp://127.0.0.1:3306', $errno, $errstr);
if ($server === false) {
    $t->assertTrue("the black-hole listener bound: {$errstr} ({$errno})", false);
    $t->done();
}
$sharedState['error_mode_dsn'] = "mysql:host=127.0.0.1;port=3306;dbname={$db}";
unset($sharedState['error_mode_entered']);

$request = function (string $path): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/hooksdb/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
};

$holder = oxphp_async($request, 'fixture_error_mode_black_hole.php');

// Until the holder is inside its constructor, and then long enough for it to be
// parked on the greeting rather than still connecting.
for ($i = 0; $i < 100 && !isset($sharedState['error_mode_entered']); $i++) {
    oxphp_usleep(20_000);
}
oxphp_usleep(200_000);

$inside = ($sharedState['error_mode_entered'] ?? false) === true;
$heard = [];
set_error_handler(static function (int $severity, string $message) use (&$heard): bool {
    $heard[] = $message;

    return true;
});
ob_start();
try {
    trigger_error('notice-neighbour', E_USER_NOTICE);
    trigger_error('warning-neighbour', E_USER_WARNING);
    $thrown = 'none';
} catch (\Throwable $e) {
    $thrown = get_class($e) . ':' . $e->getMessage();
}
$shown = (string) ob_get_clean();
restore_error_handler();
$stillInside = ($sharedState['error_mode_entered'] ?? false) === true;

// Closing the listener resets the connection the holder is parked on.
fclose($server);
$held = oxphp_async_await($holder, 10.0)['body'];

$t->assertTrue('premise: the holder was inside its constructor while this request measured', $inside && $stillInside);
$t->assertSame('this request\'s own handler hears its errors', $heard, ['notice-neighbour', 'warning-neighbour']);
$t->assertSame('and nothing is thrown in their place', $thrown, 'none');
$t->assertSame('or printed', $shown, '');
$t->assertContains('the holder\'s constructor failed on the reset connection', $held, 'black-hole-failed:PDOException:');
$t->assertNotContains(
    'and mysqlnd\'s warning about it became that PDOException rather than being printed: the holder resumed in its own mode',
    $held,
    'Warning'
);

$t->done();

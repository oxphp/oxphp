<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('promise_survives_bystander', 'hooks');

// Regression: completing an unrelated request on the same worker thread must
// not cancel this request fiber's in-flight async promise. PHP_WORKERS=1
// guarantees the bystander request below is served by THIS worker thread
// while this fiber is suspended in oxphp_async_await(). Promise cleanup used
// to be thread-scoped: the bystander's completion drained the whole
// thread-local promise map, so the await below timed out at its full 6s
// deadline even though the task itself finished successfully.
$task = oxphp_async(function (): int {
    oxphp_usleep(2000000); // 2s cooperative sleep on the async worker
    return 7;
});

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('bystander socket connected', $sock !== false);
stream_set_timeout($sock, 5);
fwrite($sock, "GET /tests/hooks/fixture_bystander_nosleep.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

$t0 = microtime(true);
$result = null;
$awaitError = '';
try {
    $result = oxphp_async_await($task, 6.0);
} catch (\Throwable $e) {
    $awaitError = get_class($e) . ': ' . $e->getMessage();
}
$elapsed = microtime(true) - $t0;

$resp = (string) stream_get_contents($sock);
fclose($sock);

$t->assertContains('bystander request was served during the await', $resp, 'bystander-done');
$t->assertSame('await raised no error (got: ' . ($awaitError ?: 'none') . ')', $awaitError, '');
$t->assertSame('task result delivered intact', $result, 7);
$t->assertTrue('await resolved when the task finished (~2s), not at the 6s deadline',
    $elapsed < 3.5);
$t->assertGreaterThan('await covered the remainder of the 2s task', $elapsed, 1.5);

$t->done();

<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('db_clients_reachable', 'hooksdb');

// Runs first in this suite so that a missing extension or an unreachable server
// reports itself plainly, instead of surfacing as an unexplained timing failure
// in the multiplexing tests that follow.
$t->assertTrue('pdo_mysql is loaded', extension_loaded('pdo_mysql'));
$t->assertTrue('phpredis is loaded', extension_loaded('redis'));

$mysqlHost = getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql';
$redisHost = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';

$pdo = new PDO(
    'mysql:host=' . $mysqlHost . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
    getenv('DB_USER') ?: 'appuser',
    getenv('DB_PASS') ?: 'apppass',
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);
// PDO_MySQL returns integer columns as PHP ints, so this is compared as one.
$t->assertSame('MySQL answers a trivial query', $pdo->query('SELECT 1')->fetchColumn(), 1);

$redis = new Redis();
$t->assertTrue('Redis accepts a connection', $redis->connect($redisHost, 6379, 3.0));
$t->assertTrue('Redis answers PING', (bool) $redis->ping());
$redis->close();

$t->done();

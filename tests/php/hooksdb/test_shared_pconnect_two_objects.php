<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_pconnect_two_objects', 'hooksdb');

// A premise, not a scenario. A claim on a phpredis connection is keyed by the
// Redis object, which is only sound while one live object means one connection —
// the same reasoning that made the PDO claim key on the driver handle instead,
// since PDO::ATTR_PERSISTENT hands one connection to every object with the same
// DSN. pconnect() reads like that and is not: phpredis pools by handing a free
// connection to one object at a time. If a future phpredis ever changed that, the
// object key would silently stop naming a connection, and this is what would say
// so — CLIENT ID being the server's own name for a connection.
//
// Both settings of the pooling switch are checked because it is the one thing
// that plausibly changes the answer, and the default is not the only value in use.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';

foreach (['1', '0'] as $pooling) {
    ini_set('redis.pconnect.pooling_enabled', $pooling);

    $first = new Redis();
    $first->pconnect($host, 6379, 3.0, "premise-{$pooling}", 0, 5.0);
    $second = new Redis();
    $second->pconnect($host, 6379, 3.0, "premise-{$pooling}", 0, 5.0);

    $t->assertNotEqual(
        "two live handles opened with identical pconnect() arguments are two connections"
            . " (redis.pconnect.pooling_enabled={$pooling})",
        $first->rawCommand('CLIENT', 'ID'),
        $second->rawCommand('CLIENT', 'ID')
    );
}

$t->done();

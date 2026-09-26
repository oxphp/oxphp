<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_gate_spellings', 'hooksdb');

// Persistent constructors wait for each other only where they can land on the
// same pool entry, and PDO's pool key is built from the data source after PDO has
// resolved it — not from the string the application passed. So one data source
// reached by two spellings is one pool entry, and the two constructors must still
// wait for each other: an alias defined in php.ini (`pdo.dsn.hooksdb`, set in this
// image), a `uri:` naming a file the DSN is read from, and an object PDO converts
// to a string. Each is raced here against the same data source spelled out in
// full; a pair that did not wait would both miss the pool, both connect, and the
// second registration would free the connection the first is using.
//
// The last pair is two different DSNs naming one pool entry: PDO joins DSN, user
// and password with colons and escapes nothing, so a DSN extended by ":x" with user
// "y" and password "p" meets the unextended one with user "x" and password "y:p"
// (the fixture spells both out). Those two must wait for each other too, whatever
// the strings say about the source.
$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$uriFile = '/tmp/oxphp-ctor-dsn-' . bin2hex(random_bytes(4));
file_put_contents($uriFile, $dsn);
$sharedState['ctor_race_uri_file'] = $uriFile;

$request = function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_ctor_race.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
};

// The two accounts the split pair authenticates as. Only oxgate_a's password
// carries a colon — the one the pair relies on; access to the profile's database
// is all they need.
$root = new PDO($dsn, 'root', 'rootpass', [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
foreach (['oxgate_a' => 'oxgate_b:pw', 'oxgate_b' => 'pw'] as $account => $password) {
    $root->exec("CREATE USER IF NOT EXISTS '{$account}'@'%' IDENTIFIED BY '{$password}'");
    $root->exec("GRANT SELECT ON appdb.* TO '{$account}'@'%'");
}
$root = null;

try {
    // Both orders for the two spellings with no source of their own: one taking its
    // turn while a named source is building, and one a named source has to wait for.
    $pairs = [
        ['alias', 'literal'],
        ['uri', 'literal'],
        ['literal', 'uri'],
        ['object', 'literal'],
        ['literal', 'object'],
        ['split-a', 'split-b'],
    ];
    foreach ($pairs as $pair) {
        $via = implode('/', $pair);
        // A fresh pool key per pair, so each starts from an empty pool.
        $sharedState['ctor_race_key'] = 'ctor-' . str_replace('/', '-', $via) . '-' . bin2hex(random_bytes(4));
        $sharedState['ctor_race_vias'] = $pair;

        $tasks = [oxphp_async($request), oxphp_async($request)];

        $ids = [];
        $spans = [];
        $failed = [];
        foreach ($tasks as $task) {
            $body = oxphp_async_await($task)['body'];
            if (preg_match('/^ctor-race-done:(\d+) (\d+\.\d+) (\d+\.\d+)$/m', $body, $m) === 1) {
                $ids[] = $m[1];
                $spans[] = [(float) $m[2], (float) $m[3]];
                continue;
            }
            foreach (explode("\n", $body) as $line) {
                if (str_starts_with($line, 'ctor-race-') || str_starts_with($line, 'HTTP/')) {
                    $failed[] = trim($line);
                }
            }
        }

        $t->assertSame("{$via}: both requests finished their own query: " . implode(' | ', $failed), count($ids), 2);

        // The premise: the two were inside PDO at the same moment. Two that ran one
        // after the other would name one connection honestly having raced over
        // nothing. On a fixed build they overlap too — waiting happens inside the
        // constructor.
        $overlapped = count($spans) === 2
            && $spans[0][0] < $spans[1][1] && $spans[1][0] < $spans[0][1];
        $t->assertTrue("{$via}: the two constructors were inside PDO at the same moment", $overlapped);

        $t->assertCount(
            "{$via}: and they share one persistent connection, not " . count(array_unique($ids)) . ' separate ones',
            array_unique($ids),
            1
        );
    }
} finally {
    unset($sharedState['ctor_race_vias'], $sharedState['ctor_race_uri_file']);
    @unlink($uriFile);
}

$t->done();

<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A .oxphpdeny denial is counted in oxphp_path_deny_total and not in
// oxphp_php_deny_total, so the two features stay apart on a dashboard. /config
// reports the file as counts only.
$t = new TestCase('test_denials_counted_and_summarized', 'oxphpdeny');

$internal = 'http://127.0.0.1:9090';

// curl rather than a stream context: `$http_response_header` is deprecated
// as of PHP 8.5 and this suite has to stay quiet on both supported versions.
$status = static function (string $url): ?int {
    $ch = curl_init($url);
    curl_setopt_array($ch, [
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_TIMEOUT => 5,
    ]);
    $ok = curl_exec($ch) !== false;
    $code = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
    return $ok ? $code : null;
};

$counter = static function (string $name) use ($internal): ?int {
    $body = @file_get_contents($internal . '/metrics');
    if (!is_string($body) || !preg_match('/^' . preg_quote($name, '/') . ' (\d+)$/m', $body, $m)) {
        return null;
    }
    return (int) $m[1];
};

$pathBefore = $counter('oxphp_path_deny_total');
$phpBefore = $counter('oxphp_php_deny_total');
$t->assertNotNull('oxphp_path_deny_total is exported', $pathBefore);
$t->assertNotNull('oxphp_php_deny_total is exported', $phpBefore);

// Denied in Rust, so this request needs no PHP worker of its own.
$t->assertSame('a denied path answers 404', $status('http://127.0.0.1/dump.sql'), 404);

$t->assertSame(
    'oxphp_path_deny_total counts the denial',
    $counter('oxphp_path_deny_total'),
    $pathBefore + 1
);
$t->assertSame(
    'oxphp_php_deny_total does not',
    $counter('oxphp_php_deny_total'),
    $phpBefore
);

$config = @file_get_contents($internal . '/config');
$decoded = is_string($config) ? json_decode($config, true) : null;
// `/config` keys arrive sorted, and `===` on arrays compares order too.
$t->assertSame('config summarizes the file', $decoded['deny_file'] ?? null, [
    'allow' => 0,
    'deny' => 5,
    'entry' => 0,
    'fallback' => '404',
    'loaded' => true,
    'rules' => 5,
]);

$t->done();

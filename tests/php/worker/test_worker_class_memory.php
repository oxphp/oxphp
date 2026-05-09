<?php
$w = OxPHP\Server\Worker::current();
$mem = $w->memoryUsage();
$rss = $w->rss();

if (!is_int($mem) || $mem <= 0) {
    http_response_code(500);
    echo "FAIL: memoryUsage() = " . var_export($mem, true) . "\n";
    exit;
}
if (!is_int($rss) || $rss <= 0) {
    http_response_code(500);
    echo "FAIL: rss() = " . var_export($rss, true) . "\n";
    exit;
}
if ($rss < $mem) {
    http_response_code(500);
    echo "FAIL: rss($rss) < memoryUsage($mem)\n";
    exit;
}
echo "OK mem=$mem rss=$rss\n";

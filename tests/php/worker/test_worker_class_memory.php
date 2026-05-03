<?php
$w = OxPHP\Server\Worker::current();
$mem = $w->getMemoryUsage();
$rss = $w->getRss();

if (!is_int($mem) || $mem <= 0) {
    http_response_code(500);
    echo "FAIL: getMemoryUsage() = " . var_export($mem, true) . "\n";
    exit;
}
if (!is_int($rss) || $rss <= 0) {
    http_response_code(500);
    echo "FAIL: getRss() = " . var_export($rss, true) . "\n";
    exit;
}
if ($rss < $mem) {
    http_response_code(500);
    echo "FAIL: getRss($rss) < getMemoryUsage($mem)\n";
    exit;
}
echo "OK mem=$mem rss=$rss\n";

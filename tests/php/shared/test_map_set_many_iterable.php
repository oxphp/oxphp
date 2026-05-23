<?php
// setMany accepts any iterable, including a Generator source.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
$gen = (function () {
    yield 'a' => 1;
    yield 'b' => 2;
})();
if ($m->setMany($gen) !== 2) {
    echo "FAIL: setMany count != 2\n";
    exit;
}
if ($m->get('a') !== 1 || $m->get('b') !== 2) {
    echo "FAIL: setMany values wrong\n";
    exit;
}
echo "OK\n";

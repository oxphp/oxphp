<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/ini_put_back_probe.php';

// Reads what fibers/test_ini_put_back_does_not_park left. The neighbour it sent
// may still be queued when this request arrives; this one parks until it has
// run.

$t = new TestCase('ini_put_back_does_not_park', 'fibers');

$until = microtime(true) + 3.0;
while (!OxphpIniPutBackProbe::$neighbourRan && microtime(true) < $until) {
    oxphp_sleep(0.05);
}

if (is_resource(OxphpIniPutBackProbe::$sock)) {
    fclose(OxphpIniPutBackProbe::$sock);
}
OxphpIniPutBackProbe::$sock = null;

// The premise: putting the directive back is what ran the destructor.
$t->assertTrue('putting assert.callback back ran the destructor', OxphpIniPutBackProbe::$destructorRan);
$t->assertTrue('the neighbour it sent ran', OxphpIniPutBackProbe::$neighbourRan);
$t->assertSame(
    'and only after the destructor had finished sleeping: it did not park',
    OxphpIniPutBackProbe::$neighbourSawDestructor,
    false
);

$t->done();

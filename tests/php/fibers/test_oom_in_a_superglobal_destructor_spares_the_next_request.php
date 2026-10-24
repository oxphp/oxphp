<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// fibers/test_oom_in_a_superglobal_destructor_spares_a_parked_request with
// nothing else in flight on the worker: the destructor that runs out of memory
// is the last thing the worker runs for this request, and the next thing it does
// is start the request after it — fibers/..._spares_the_next_request_probe,
// which says how that went. What this one leaves for it is the worker it ran on
// and the mark the destructor leaves before it runs out.

$t = new TestCase('oom_in_a_superglobal_destructor_spares_the_next_request', 'fibers');

$base = sys_get_temp_dir() . '/oxphp-superglobal-oom-next';
if (is_file("$base.ran")) {
    unlink("$base.ran");
}
file_put_contents("$base.worker", sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()));

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file.
$_GET['held'] = new class ("$base.ran") {
    public function __construct(private string $marker)
    {
    }

    public function __destruct()
    {
        file_put_contents($this->marker, 'ran');
        $s = 'x';
        for (;;) {
            $s .= $s;
        }
    }
};

$t->assertTrue('the object is left in $_GET', isset($_GET['held']));

$t->done();

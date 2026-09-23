<?php

declare(strict_types=1);

// The neighbour for fibers/test_ini_put_back_does_not_park: says whether the
// request that sent it was still inside its destructor when this one ran.

require_once __DIR__ . '/ini_put_back_probe.php';

OxphpIniPutBackProbe::$neighbourSawDestructor = OxphpIniPutBackProbe::$inDestructor;
OxphpIniPutBackProbe::$neighbourRan = true;

echo 'neighbour';

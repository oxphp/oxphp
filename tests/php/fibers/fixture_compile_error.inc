<?php

// A file that fails to compile: included, it fatals with E_COMPILE_ERROR before
// the class is bound, so every include of it fatals the same way. Named .inc so
// that nothing takes it for a script to run.

abstract class OxphpCompileErrorProbe
{
    abstract public function f()
    {
    }
}

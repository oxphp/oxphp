<?php

declare(strict_types=1);

// A request that ends in an uncaught application exception: the ordinary way a
// PHP application reports a failure it did not expect. The engine unwinds
// cleanly, the worker answers 500, and nothing about the worker itself is
// damaged — so a run of these must not retire it.
//
// No TestCase: its constructor installs an exception handler, which would
// catch this and turn the request into a normal one.

throw new RuntimeException('breaker: uncaught application exception');

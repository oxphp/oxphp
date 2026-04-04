<?php

declare(strict_types=1);

// No TestCase — the uncaught exception IS the test.
// The runner expects HTTP 500. The script must not produce valid JSON output.
throw new \RuntimeException('Intentional test exception');

<?php
declare(strict_types=1);
// A request that leaves an output buffer open. Its content is this response's
// body: closing the buffer is part of ending this request, not of starting
// whichever request the worker serves next.
ob_start();
echo "buffered-by-its-own-request\n";

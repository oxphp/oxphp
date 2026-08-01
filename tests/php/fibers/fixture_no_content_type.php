<?php
declare(strict_types=1);
// A response that sets no Content-Type of its own, so the engine supplies the
// default one. That default is allocated per request and handed to the request
// to give back; a worker that never gives it back grows by one string for every
// request of this shape — which is most of them.
echo "ok\n";

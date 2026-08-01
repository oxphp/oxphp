<?php
declare(strict_types=1);
// A response that sets a header of its own and writes nothing. The engine still
// supplies a Content-Type for it, the way it does under every other SAPI.
header('X-Probe: 1');

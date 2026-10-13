<?php
// `oxphp run` runs the script as one request: a pool bound to a name serves the
// whole script, and the slot it leaves idle is let go with the request, before
// the server shuts the shared registry down.
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

$pool = Registry::pool(
    'cli-pool',
    static fn (): Pool => new Pool(factory: static fn (): \ArrayObject => new \ArrayObject(['marker' => 'pool-ok']), destroy: null, maxSize: 1)
);
$h = $pool->acquire();
$marker = $h->get()['marker'];
$h->release();
echo $marker, ' idle=', $pool->stats()->idle(), "\n";

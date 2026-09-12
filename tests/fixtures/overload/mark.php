<?php
// Leaves a trace that it ran, outside the read-only document root.
//
// Used as the target of selfcall.php's inner call: a request the waiting side
// refused on its queue deadline is still sitting in the queue, and the worker
// that eventually reaches it must drop it rather than run it — its client was
// answered a second ago. Nothing in a response can show that, because the
// response of such a run goes to a channel nobody is reading. The mark can.
file_put_contents('/tmp/oxphp-inner-ran', 'x', FILE_APPEND);
header('Content-Type: text/plain');
echo "marked\n";

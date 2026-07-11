<?php
// Manual path: oxphp_apm_error($e) records the exception onto an explicit span.
$id = oxphp_apm_start('manual_span');
try {
    throw new LogicException('manual path: bad state');
} catch (\Throwable $e) {
    oxphp_apm_error($e, $id);
}
oxphp_apm_end($id);
echo "manual ok\n";

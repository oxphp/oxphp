<?php
// Manual path, bare string reason: oxphp_apm_error('reason') with no Throwable
// records the string as exception.message under a generic "Error" type so the
// event stays visible in type-keyed error backends.
$id = oxphp_apm_start('reason_span');
oxphp_apm_error('reason path: gateway timeout', $id);
oxphp_apm_end($id);
echo "reason ok\n";

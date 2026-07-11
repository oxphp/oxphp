<?php
// A Throwable passed through a PHP reference. oxphp_apm_error's parameter is
// by-value, so the VM dereferences the reference (ZVAL_COPY_DEREF in SEND_VAR)
// before the capture ever sees the argument slot — it reads IS_OBJECT, not
// IS_REFERENCE. This pins that a reference-to-Throwable is still recorded.
$id = oxphp_apm_start('ref_span');
$e = new RuntimeException('ref path: boom');
$ref = &$e;
oxphp_apm_error($ref, $id);
oxphp_apm_end($id);
echo "ref ok\n";

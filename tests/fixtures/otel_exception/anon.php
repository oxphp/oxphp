<?php
// Anonymous-class exception: its class name embeds a NUL
// ("<parent>@anonymous\0<file>:<line>$<hash>"). The capture must carry the name
// length-delimited (not truncated at the NUL) so distinct anonymous classes
// stay distinguishable in exception.type; the NUL is then stripped for output.
$id = oxphp_apm_start('anon_span');
try {
    throw new class('anon path: boom') extends \RuntimeException {};
} catch (\Throwable $e) {
    oxphp_apm_error($e, $id);
}
oxphp_apm_end($id);
echo "anon ok\n";

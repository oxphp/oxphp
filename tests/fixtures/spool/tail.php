<?php
// 48 MiB written in 64 KiB pieces. With RESPONSE_BUFFER_BYTES at 32 MiB the
// output crosses the threshold once, and the last 16 MiB are still buffered
// when the script ends.
header('Content-Type: application/octet-stream');
$h = fopen('/tmp/big.bin', 'rb');
for ($i = 0; $i < 768; $i++) {
    echo fread($h, 65536);
}

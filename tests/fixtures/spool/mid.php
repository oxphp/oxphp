<?php
// 3 MiB written in 8 KiB pieces: the response crosses the in-memory threshold
// through many small writes rather than one large one.
header('Content-Type: application/octet-stream');
$h = fopen('/tmp/mid.bin', 'rb');
while (!feof($h)) {
    echo fread($h, 8192);
}

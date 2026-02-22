<?php

$kb    = min((int)($_GET['kb'] ?? 64), 4096);
$chunk = str_repeat('x', 1024);
header('Content-Type: text/plain');
header('Content-Length: ' . ($kb * 1024));
for ($i = 0; $i < $kb; $i++) {
    echo $chunk;
}

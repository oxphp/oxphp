<?php
// A script one directory down: `*` stops at `/`, so `/uploads/*.php` misses
// it and `/uploads/**/*.php` does not.
echo "executed";

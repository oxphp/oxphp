<?php
declare(strict_types=1);
// Throwaway: what the worker's Zend allocator holds right now.
header('Content-Type: text/plain');
echo 'real=', memory_get_usage(true), ' used=', memory_get_usage(false), "\n";

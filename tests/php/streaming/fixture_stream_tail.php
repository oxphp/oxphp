<?php

declare(strict_types=1);

// Inner request for streaming/test_stream_tail_after_the_last_flush_arrives:
// a stream whose last write is never flushed by the script itself.

echo 'HEAD1';
oxphp_stream_flush();
echo 'TAIL';

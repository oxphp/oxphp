<?php

declare(strict_types=1);

// Served in its own request fiber, and says when: a test asks for it while its
// own request is waiting, to tell whether the worker went on serving meanwhile.
echo 'served-at:' . sprintf('%.6F', microtime(true));

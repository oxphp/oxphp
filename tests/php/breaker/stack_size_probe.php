<?php

// Answers only from inside its handler, so a request that could not run its
// handler on the fiber it was given cannot produce this body.
echo "stack-size-probe-ok\n";

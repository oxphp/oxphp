<?php

// A file that compiles, raising a deprecation while it does: the parameter's
// null default makes its type nullable implicitly. Named .inc so that nothing
// takes it for a script to run.

return static function (int $x = null): void {
};

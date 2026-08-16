<?php

// Nothing but a require, and that is the whole point: a file with no variables
// of its own compiles to `last_var == 0`, and the engine never hands such a
// script the symbol table to hold. A fatal underneath it therefore has to hand
// the variables back past this frame rather than to it — one level further than
// the direct case, and the shape of every `require 'bootstrap.php';` there is.
require __DIR__ . '/fixture_shared_state_probe.php';

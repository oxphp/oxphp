<?php
// Probe for SUPERGLOBALS_ENABLED behaviour under `oxphp run`.
//   line 1: argc + first argv entry  -> proves $argv survives the flag
//   line 2: skel-yes|skel-no          -> $_SERVER script skeleton survives
//   line 3: env-yes|env-no            -> process-env fold into $_SERVER (gated)
echo $argc, "|", $argv[0], "\n";
echo isset($_SERVER['SCRIPT_NAME']) ? "skel-yes\n" : "skel-no\n";
echo isset($_SERVER['OXPHP_SG_PROBE']) ? "env-yes\n" : "env-no\n";

<?php
// A handler that names its own fatal exactly as the server names the one it
// raises to unwind a cancelled request. Nobody cancelled this request, so the
// quietening must not apply: a script that could reach it by text alone would
// be able to hide any fatal it liked.
//
// log_errors is off in the image, and one of the two routes this fatal takes
// to the log is PHP's own error log. Switch it on so both are exercised.
ini_set('log_errors', '1');

trigger_error('Request cancelled (client_abort)', E_USER_ERROR);

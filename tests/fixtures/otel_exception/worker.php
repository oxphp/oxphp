<?php
// Worker-mode entry: the handler throws on /boom. The fiber harness catches the
// uncaught exception (it never reaches zend_exception_error), so OxPHP captures
// it at the catch site and the root SERVER span still carries an exception event.
oxphp_worker(function () {
    if (($_SERVER['REQUEST_URI'] ?? '/') === '/boom') {
        throw new RuntimeException('worker path: handler exploded');
    }
    header('Content-Type: text/plain');
    echo 'ok';
});

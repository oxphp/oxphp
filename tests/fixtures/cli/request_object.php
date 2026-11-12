<?php
// `oxphp run` serves no HTTP request: the factory throws, and so does a method
// of a Request built with `new`.
$calls = [
    static fn() => oxphp_http_request(),
    static fn() => (new \OxPHP\Http\Request())->method(),
];
foreach ($calls as $call) {
    try {
        $result = $call();
        echo 'no-throw: ', json_encode($result), ';';
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), ';';
    }
}

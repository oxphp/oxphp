<?php

/**
 * The inner-request helper the session tests share.
 *
 * Pulled in with require_once for the reason fiber_park_registry.php gives: in
 * worker mode a function declared by a test file is declared on the worker, not
 * on the request, so two tests carrying a copy of the same helper would be the
 * second one failing to declare it.
 */

declare(strict_types=1);

/**
 * Issues a request over a fresh connection, carrying $cookie as the session
 * cookie, and returns its body.
 *
 * Same shape as fiber_inner_request() in fiber_park_registry.php, with the one
 * addition the session tests need — the cookie, which is the input a leaked
 * session makes the server ignore. The read is what parks this fiber
 * (RUNTIME_HOOKS covers a blocking read on a tcp:// stream), which is both what
 * frees the worker to pick the inner request up and what leaves this one parked
 * while it runs. With PHP_WORKERS=1 a body coming back at all proves the inner
 * request was served by the thread this call is running on.
 */
function session_inner_request(string $path, string $cookie, float $timeout = 5.0): string
{
    return session_inner_read(session_inner_send($path, $cookie, $timeout), $timeout);
}

/**
 * Sends the request and hands back the socket without reading it.
 *
 * Splitting the send from the read is what lets a test put more than one inner
 * request on the worker at a time: the caller keeps running until it reads, and
 * reading is what parks it. A test that needs two requests to overlap sends both
 * and reads them in the order it wants them to finish.
 *
 * @return resource
 */
function session_inner_send(string $path, string $cookie, float $timeout = 5.0)
{
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, $timeout);
    if ($sock === false) {
        throw new \RuntimeException("inner connect failed: $errstr ($errno)");
    }

    stream_set_timeout($sock, (int) ceil($timeout));
    fwrite($sock, "GET $path HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\n"
        . "Cookie: PHPSESSID=$cookie\r\n"
        . "Connection: close\r\n\r\n");

    return $sock;
}

/**
 * Reads a socket session_inner_send() returned and gives back the body.
 *
 * @param resource $sock
 */
function session_inner_read($sock, float $timeout = 5.0): string
{
    stream_set_timeout($sock, (int) ceil($timeout));
    $raw = (string) stream_get_contents($sock);
    fclose($sock);

    $split = strpos($raw, "\r\n\r\n");

    return $split === false ? $raw : substr($raw, $split + 4);
}

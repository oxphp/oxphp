<?php
// Chained exception: the request is killed by the OUTER DomainException, whose
// `previous` is a PDOException (the root cause). PHP renders the "Uncaught …"
// message root-cause-first, so the span must still bucket on the thrown
// DomainException — its type and message, not the root cause's — with the whole
// chain kept in the stacktrace.
function dbConnect(): void {
    throw new PDOException('chained cause: db unreachable');
}

function apiHandle(): void {
    try {
        dbConnect();
    } catch (\Throwable $prev) {
        throw new DomainException('chained outer: api failed', 0, $prev);
    }
}

apiHandle();

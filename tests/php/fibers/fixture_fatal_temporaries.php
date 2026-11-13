<?php

declare(strict_types=1);

// A fatal raised while the frames it leaves behind hold values that are not
// variables: the subject of a foreach, the left operand of an expression whose
// right one is still being computed, the arguments already passed to a call that
// has not started, the part of a result an internal function had built, the
// pieces of an interpolated string. The engine gives those back as each finishes,
// so a fatal that stops them first leaves them to whoever releases the frames —
// on a worker, for as long as the worker lives.
//
// Measured by tests/scripts/verify_worker_does_not_grow.sh, one ?form= per
// shape, against ?form=bare: the same fatal with nothing in flight. Each form
// holds about a kilobyte, made at run time so that none is a literal the worker
// keeps anyway — all but rope_init, which fatals before it holds anything and is
// there for the release not to free a piece that was never written.

// Declared conditionally: the worker keeps this process across requests.
if (!function_exists('oxphp_tmp_fatal')) {
    // The fatal, raised with nothing being compiled, as in fixture_fatal.php.
    // Called twice so that it fatals on a worker no earlier request has
    // declared the class on.
    function oxphp_tmp_fatal(): void
    {
        class OxphpTmpProbeFatal
        {
        }
    }

    function oxphp_tmp_fatal_string(): string
    {
        oxphp_tmp_fatal();
        oxphp_tmp_fatal();
        return '';
    }

    function oxphp_tmp_blob(): string
    {
        return bin2hex(random_bytes(500));
    }

    /** @return list<int> */
    function oxphp_tmp_rows(): array
    {
        return range(1, 64 + random_int(0, 0));
    }

    function oxphp_tmp_foreach(): void
    {
        foreach (oxphp_tmp_rows() as $row) {
            oxphp_tmp_fatal_string();
        }
    }

    function oxphp_tmp_operand(): string
    {
        return oxphp_tmp_blob() . oxphp_tmp_fatal_string();
    }

    /** @return array<string, string> */
    function oxphp_tmp_icall(): array
    {
        // String keys: on 8.5 a packed input is filled into slots the array
        // does not count until the call returns, which nothing can release.
        $input = [];
        for ($i = 1; $i <= 9; $i++) {
            $input["k{$i}"] = $i;
        }
        return array_map(static function (int $i): string {
            if ($i === 9) {
                oxphp_tmp_fatal_string();
            }
            return str_repeat('m', 100 + random_int(0, 0));
        }, $input);
    }

    function oxphp_tmp_pending(OxphpTmpTaker $taker): void
    {
        $taker->take(oxphp_tmp_blob(), oxphp_tmp_fatal_string());
    }

    function oxphp_tmp_trampoline(OxphpTmpMagic $magic): void
    {
        $magic->{'m' . oxphp_tmp_blob()}(oxphp_tmp_blob(), oxphp_tmp_fatal_string());
    }

    // One object for every request: the engine takes a reference to an object
    // for the call to its __toString() and drops it when the call returns, so a
    // fatal in there keeps the object for the life of the worker. No frame holds
    // that reference, and the release of frames is what the rope forms measure.
    function oxphp_tmp_fatal_object(): OxphpTmpFatalString
    {
        static $object = null;
        return $object ??= new OxphpTmpFatalString();
    }

    // The fatal while a piece in the middle is added, while the first one is,
    // and while the last one is and the string is put together.
    function oxphp_tmp_rope(): string
    {
        $text = oxphp_tmp_blob();
        $fatal = oxphp_tmp_fatal_object();
        return "<{$text}|{$fatal}>";
    }

    // $joined first: the string it builds is laid out by the optimizer so that
    // its last piece, $text, stays in the slot the interpolation after it starts
    // in. A first piece the release frees without its ever being written is then
    // a reference $text did not give, and the worker frees $text twice.
    function oxphp_tmp_rope_init(): string
    {
        $text = oxphp_tmp_blob();
        $fatal = oxphp_tmp_fatal_object();
        $joined = "{$text}|{$text}";
        return "{$fatal}|{$joined}";
    }

    function oxphp_tmp_rope_end(): string
    {
        $text = oxphp_tmp_blob();
        $fatal = oxphp_tmp_fatal_object();
        return "{$text}|{$fatal}";
    }

    final class OxphpTmpTaker
    {
        public function take(string ...$args): void
        {
        }
    }

    final class OxphpTmpMagic
    {
        /** @param list<mixed> $args */
        public function __call(string $name, array $args): void
        {
        }
    }

    final class OxphpTmpFatalString
    {
        public function __toString(): string
        {
            return oxphp_tmp_fatal_string();
        }
    }
}

header('Content-Type: text/plain');

// Dispatched on a variable: a switch or a match on the expression itself would
// hold its value in a temporary while a case runs, a second value in flight in
// every form below.
$form = $_GET['form'] ?? '';
if ($form === 'bare') {
    oxphp_tmp_fatal_string();
} elseif ($form === 'foreach') {
    oxphp_tmp_foreach();
} elseif ($form === 'top') {
    // The same as foreach, but in the included file's own code rather than in
    // a function it calls.
    foreach (oxphp_tmp_rows() as $row) {
        oxphp_tmp_fatal_string();
    }
} elseif ($form === 'operand') {
    oxphp_tmp_operand();
} elseif ($form === 'icall') {
    oxphp_tmp_icall();
} elseif ($form === 'pending') {
    oxphp_tmp_pending(new OxphpTmpTaker());
} elseif ($form === 'trampoline') {
    oxphp_tmp_trampoline(new OxphpTmpMagic());
} elseif ($form === 'rope') {
    oxphp_tmp_rope();
} elseif ($form === 'rope_init') {
    oxphp_tmp_rope_init();
} elseif ($form === 'rope_end') {
    oxphp_tmp_rope_end();
}

echo "NOT-REACHED\n";

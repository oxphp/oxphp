<?php

declare(strict_types=1);

// Shared by the tests of array_multisort() on a worker. The sort keeps its table
// of comparison functions on the worker thread for the whole call, and what a
// comparison runs — a __toString() under SORT_STRING here — runs inside that
// window.
//
// Pulled in with require_once: a worker keeps what a file declares for as long
// as it lives, and a second plain require would declare it again.

if (!class_exists('MultisortProbeValue', false)) {
    /** A string array_multisort() reaches through __toString(), whose first cast runs a hook. */
    final class MultisortProbeValue
    {
        public function __construct(private string $value, private ?\Closure $onFirstCast = null)
        {
        }

        public function __toString(): string
        {
            $hook = $this->onFirstCast;
            $this->onFirstCast = null;
            if ($hook !== null) {
                $hook();
            }

            return $this->value;
        }
    }
}

/**
 * Sorts three strings with SORT_STRING, and a second array by them. The hook
 * runs on the first comparison, with more of them still to come.
 *
 * @return array{strings: list<string>, order: list<int>}
 */
function multisort_probe_sort(\Closure $onFirstCast): array
{
    $values = [new MultisortProbeValue('c', $onFirstCast), new MultisortProbeValue('a'), new MultisortProbeValue('b')];
    $order = [3, 1, 2];
    array_multisort($values, SORT_STRING, $order);

    return ['strings' => array_map('strval', $values), 'order' => $order];
}

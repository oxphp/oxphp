<?php

/**
 * Shared between the two halves of the fast-path session check.
 *
 * Pulled in with require_once, so in worker mode it outlives a single request
 * and both halves see the same class — which is the whole point: the second half
 * has to be able to say that the first one really ran on this worker before it.
 * Without that, a run where the two were reordered, or where only the second was
 * selected, would pass having exercised nothing at all.
 */

declare(strict_types=1);

final class SessionFastPathLatch
{
    private static ?string $seededId = null;

    public static function seeded(string $id): void
    {
        self::$seededId = $id;
    }

    public static function seededId(): ?string
    {
        return self::$seededId;
    }
}

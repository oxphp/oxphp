<?php
// The ox_shared classes (OxPHP\Shared\*) must be registered under the one-shot
// CLI role, proving the engine plugins — not just bare PHP — are available.
$shared = array_values(array_filter(
    get_declared_classes(),
    static fn($c) => str_starts_with($c, 'OxPHP\\Shared\\')
));
sort($shared);
echo "count:", count($shared), "\n";
echo count($shared) > 0 ? "shared-ok\n" : "shared-none\n";

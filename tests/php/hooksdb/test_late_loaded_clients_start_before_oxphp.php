<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('late_loaded_clients_start_before_oxphp', 'hooksdb');

// Under RUNTIME_HOOKS=streams the database entry points are guarded from
// oxphp_sapi's own module startup, and the methods among them only on the client
// classes that exist by then. Extensions loaded from ini files start in the order
// those files are read unless a declared dependency rearranges them, so a client
// loaded from a file that sorts after oxphp's would start later and leave the
// methods of its class unguarded for the life of the process — mysqli's procedural
// functions are registered when the extension is loaded, and stay guarded either
// way. This profile's image loads mysqli and redis from such files; the premise
// below says that is still the case, and the rest says both started first anyway.

// Which scanned ini file loads which extension, in the order PHP read them. The
// first file to name one is the one it is loaded from; a later one only warns.
$loadedFrom = [];
foreach (array_filter(array_map('trim', explode(',', (string) php_ini_scanned_files()))) as $i => $file) {
    foreach (file($file, FILE_IGNORE_NEW_LINES) ?: [] as $line) {
        if (preg_match('/^\s*extension\s*=\s*"?([^"\s;]+)/', $line, $m)) {
            $loadedFrom[basename($m[1], '.so')] ??= $i;
        }
    }
}

$t->assertKeyExists('an ini file loads oxphp_sapi', $loadedFrom, 'oxphp_sapi');

// get_loaded_extensions() lists the module registry in the order the modules
// were started.
$started = array_flip(get_loaded_extensions());

foreach (['mysqli', 'redis'] as $client) {
    $t->assertKeyExists("an ini file loads $client", $loadedFrom, $client);
    $t->assertGreaterThan(
        "$client is loaded from an ini file read after oxphp's",
        $loadedFrom[$client] ?? -1,
        $loadedFrom['oxphp_sapi'] ?? PHP_INT_MAX
    );
    $t->assertLessThan(
        "$client started before oxphp_sapi",
        $started[$client] ?? PHP_INT_MAX,
        $started['oxphp_sapi'] ?? -1
    );
}

// The order above is the outcome, and with two clients loaded late it cannot pin
// each declaration by itself: PHP satisfies a dependency by swapping the two modules,
// so the one on the client read last moves oxphp_sapi past the modules read between
// oxphp's file and that client's, and those whose own dependencies all load earlier
// stay ahead of it — the other client among them. A missing entry for the client
// read first would still start it first here, and would not in a layout where it is
// the only one loaded late. So each entry is checked as declared, too.
$deps = (new ReflectionExtension('oxphp_sapi'))->getDependencies();
foreach (['pdo', 'mysqli', 'redis'] as $name) {
    $t->assertSame("oxphp_sapi declares it starts after $name", $deps[$name] ?? null, 'Optional');
}

$t->done();

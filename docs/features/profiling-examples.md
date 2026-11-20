---
title: Profiling Examples
description: Complete PHP examples for the OxPHP profiler: manual capture, attributes on services, batch jobs, A/B comparisons, self-profiling tests, custom decorators.
---

# Profiling Examples

Complete PHP scenarios for the built-in profiler. The
[profiling guide](profiling.md) explains the triggers and the SDK they use,
and the [Profiler Reference](profiling-reference.md) the settings and files.

## Practical examples (full code)

Below are ready-to-run PHP scenarios you can drop into `www/public/` and
hit with curl.

### Simple controller with manual control

```php
<?php
// www/public/report.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric, is_active};

function fetch_rows(PDO $db, int $user_id): array
{
    $stmt = $db->prepare('SELECT * FROM orders WHERE user_id = ? LIMIT 1000');
    $stmt->execute([$user_id]);
    return $stmt->fetchAll(PDO::FETCH_ASSOC);
}

function render_report(array $rows): string
{
    $sum = array_sum(array_column($rows, 'amount'));
    return json_encode(['count' => count($rows), 'total' => $sum]);
}

$db = new PDO('mysql:host=db;dbname=app', 'app', 'secret');
$user_id = (int) ($_GET['user_id'] ?? 1);

// Explicitly profile only the heavy block — even if the trigger wasn't set.
start();

mark('report.begin', ['user_id' => (string) $user_id]);

$rows = fetch_rows($db, $user_id);
metric('db.rows', (float) count($rows));

$body = render_report($rows);
metric('response.bytes', (float) strlen($body));

mark('report.done');
stop();

header('Content-Type: application/json');
echo $body;

// Optional — tell the frontend that this request was profiled:
if (is_active()) {
    header('X-Profiled: 1');
}
```

Invoke:

```bash
curl -H "X-OxPHP-Profile: dev-secret" 'http://localhost/report.php?user_id=42'
```

### Service class with attributes

```php
<?php
// www/lib/OrderService.php
declare(strict_types=1);

use OxPHP\Profile\{SlowThreshold, MemoryThreshold};

final class OrderService
{
    public function __construct(
        private readonly PDO $db,
        private readonly Mailer $mailer,
    ) {}

    #[SlowThreshold(ms: 250)]
    public function create(array $payload): int
    {
        $this->db->beginTransaction();
        try {
            $id = $this->insertOrder($payload);
            $this->insertLines($id, $payload['items']);
            $this->db->commit();
            $this->mailer->sendReceipt($id);
            return $id;
        } catch (\Throwable $e) {
            $this->db->rollBack();
            throw $e;
        }
    }

    #[MemoryThreshold(kb: 2048)]
    public function exportCsv(int $user_id): string
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE user_id = ?');
        $stmt->execute([$user_id]);

        $buf = fopen('php://temp', 'r+');
        fputcsv($buf, ['id', 'created_at', 'total']);
        while ($row = $stmt->fetch(PDO::FETCH_ASSOC)) {
            fputcsv($buf, [$row['id'], $row['created_at'], $row['total']]);
        }
        rewind($buf);
        return stream_get_contents($buf);
    }

    private function insertOrder(array $p): int { /* ... */ return 0; }
    private function insertLines(int $id, array $items): void { /* ... */ }
}
```

### Batch job: profile only the first iteration of N

```php
<?php
// www/bin/import.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark};

$files = glob('/data/incoming/*.csv');
$i = 0;

foreach ($files as $path) {
    if ($i === 0) {
        start();                       // profile only the first file in full
        mark('batch.begin', ['path' => $path]);
    }

    import_one($path);

    if ($i === 0) {
        mark('batch.first_done');
        stop();
    }
    $i++;
}

function import_one(string $path): void { /* ... */ }
```

### Compare two implementations — micro-benchmark with profiles

```php
<?php
// www/public/bench.php — naive vs streaming comparison
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric};

function naive_sum(string $path): int
{
    $rows = array_map('str_getcsv', file($path));           // whole file into memory
    return array_sum(array_column($rows, 1));
}

function streaming_sum(string $path): int
{
    $h = fopen($path, 'r');
    $total = 0;
    while (($row = fgetcsv($h)) !== false) {
        $total += (int) ($row[1] ?? 0);
    }
    fclose($h);
    return $total;
}

$path = '/data/big.csv';
$which = $_GET['impl'] ?? 'naive';

start();
mark('bench.begin', ['impl' => $which]);
$t0 = hrtime(true);

$result = $which === 'naive' ? naive_sum($path) : streaming_sum($path);

$elapsed_ms = (hrtime(true) - $t0) / 1e6;
metric('bench.elapsed_ms', $elapsed_ms);
metric('bench.result',     (float) $result);
mark('bench.done');
stop();

echo json_encode(['impl' => $which, 'elapsed_ms' => $elapsed_ms, 'result' => $result]);
```

Workflow:

```bash
# Naive
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=naive"

# Streaming
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=streaming"

# The two latest runs: download their .collapsed files and diff them (see "Validate a before/after hypothesis" in profiling.md)
curl -sH "Authorization: Bearer dev-secret" \
     "http://localhost:9090/__profiler/runs?limit=2" | jq '.runs[] | .run_id'
```

### Conditional profiling in production code

```php
<?php
// Classic case: a suspected function is slow for certain users only.
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, is_active};

function charge(User $user, Money $amount): PaymentResult
{
    // Audit: if this request is being profiled,
    // enable extra logging inside the third-party call.
    $verbose = is_active();

    $gateway = new StripeClient(verbose: $verbose);
    return $gateway->charge($user->id, $amount);
}

function oncall_path(Order $order): void
{
    // Profile only VIP users — no external trigger needed.
    if ($order->user->tier === 'vip') {
        start();
    }
    process($order);
    if ($order->user->tier === 'vip') {
        stop();
    }
}
```

### Integration test that profiles itself

```php
<?php
// tests/php/profile_smoke.php
declare(strict_types=1);
require __DIR__ . '/test_helper.php';

use function OxPHP\Profile\{start, stop, mark, is_active};

$t = new TestCase('profile_smoke', 'my-app');

// Turn profiling on from PHP — no trigger needed to test the SDK, but the
// profiler must be enabled (PROFILER_ENABLED=true), or start() does nothing.
$t->assertFalse('initially not active', is_active());
start();
$t->assertTrue('active after start', is_active());

mark('test.midpoint');

// some work
$sum = 0;
for ($i = 0; $i < 100_000; $i++) { $sum += $i; }

stop();
$t->assertFalse('inactive after stop', is_active());
$t->assertSame('computation OK', $sum, 4999950000);

$t->done();
```

### Finding a hotspot from a series of Postman requests

Scenario: "/api/search is sometimes slow, not always."

```javascript
// Postman Pre-request Script
pm.request.headers.add({
    key: 'X-OxPHP-Profile',
    value: pm.environment.get('PROFILE_TOKEN')
});
```

After 100 runs, a `jq` one-liner for top anomalies:

```bash
curl -sH "Authorization: Bearer $TOK" \
     "http://int.app.local:9090/__profiler/runs?limit=200" \
  | jq -r '.runs
          | map(select(.url | startswith("/api/search")))
          | sort_by(-.duration_ms)
          | .[:5]
          | map("\(.duration_ms)ms  \(.run_id)  \(.url)")
          | .[]'
```

### A custom decorator that feeds the profiler

Your own `#[ProfileDb]` — logs row count and automatically calls
`metric('db.rows', …)`:

```php
<?php
use OxPHP\Decorator\{AttributeInterface, Context};
use function OxPHP\Profile\metric;

#[Attribute(Attribute::TARGET_METHOD)]
class ProfileDb implements AttributeInterface
{
    public function before(Context $ctx): void {}

    public function after(Context $ctx): void
    {
        $result = $ctx->returnValue;
        if (is_array($result)) {
            metric('db.rows', (float) count($result));
        } elseif ($result instanceof PDOStatement) {
            metric('db.rows', (float) $result->rowCount());
        }
    }
}

oxphp_register_decorator(ProfileDb::class);

class UserRepository
{
    #[ProfileDb]
    public function findAll(): array { /* ... */ return []; }
}
```

The decorator + profiler pairing works out of the box: `metric()`
automatically attaches to the span of the function the Observer API is
currently watching.

---

## See Also

- [Profiling PHP code](profiling.md) — the practical guide
- [Profiler Reference](profiling-reference.md) — settings, exports and routes

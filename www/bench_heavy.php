<?php
// Simulate real application workload: class definitions, string processing, array ops

class Request {
    public string $method;
    public string $path;
    public array $headers;
    public array $query;

    public function __construct(string $method, string $path, array $headers = [], array $query = []) {
        $this->method = $method;
        $this->path = $path;
        $this->headers = $headers;
        $this->query = $query;
    }

    public function header(string $name): ?string {
        return $this->headers[strtolower($name)] ?? null;
    }
}

class Response {
    private int $status;
    private array $headers = [];
    private string $body;

    public function __construct(int $status = 200, string $body = '', array $headers = []) {
        $this->status = $status;
        $this->body = $body;
        $this->headers = $headers;
    }

    public function withHeader(string $name, string $value): self {
        $clone = clone $this;
        $clone->headers[$name] = $value;
        return $clone;
    }

    public function toArray(): array {
        return ['status' => $this->status, 'headers' => $this->headers, 'body_len' => strlen($this->body)];
    }
}

class Router {
    private array $routes = [];

    public function add(string $method, string $pattern, callable $handler): void {
        $this->routes[] = compact('method', 'pattern', 'handler');
    }

    public function dispatch(Request $request): Response {
        foreach ($this->routes as $route) {
            if ($route['method'] === $request->method && preg_match($route['pattern'], $request->path, $matches)) {
                return ($route['handler'])($request, $matches);
            }
        }
        return new Response(404, 'Not Found');
    }
}

class UserRepository {
    private array $users;

    public function __construct() {
        $this->users = [];
        for ($i = 0; $i < 50; $i++) {
            $this->users[] = [
                'id' => $i,
                'name' => 'User ' . $i,
                'email' => "user{$i}@example.com",
                'role' => $i % 3 === 0 ? 'admin' : 'user',
                'score' => random_int(0, 1000),
            ];
        }
    }

    public function findById(int $id): ?array {
        foreach ($this->users as $user) {
            if ($user['id'] === $id) return $user;
        }
        return null;
    }

    public function findByRole(string $role): array {
        return array_filter($this->users, fn($u) => $u['role'] === $role);
    }

    public function topScorers(int $limit): array {
        $sorted = $this->users;
        usort($sorted, fn($a, $b) => $b['score'] <=> $a['score']);
        return array_slice($sorted, 0, $limit);
    }
}

// --- simulate request processing ---

$router = new Router();

$router->add('GET', '#^/api/users/(\d+)$#', function(Request $req, array $m): Response {
    $repo = new UserRepository();
    $user = $repo->findById((int)$m[1]);
    return $user
        ? (new Response(200, json_encode($user)))->withHeader('Content-Type', 'application/json')
        : new Response(404, '{"error":"not found"}');
});

$router->add('GET', '#^/api/users$#', function(Request $req): Response {
    $repo = new UserRepository();
    $role = $req->query['role'] ?? null;
    $users = $role ? $repo->findByRole($role) : $repo->topScorers(10);
    return (new Response(200, json_encode(array_values($users))))->withHeader('Content-Type', 'application/json');
});

$request = new Request('GET', '/api/users', ['host' => 'localhost'], ['role' => 'admin']);
$response = $router->dispatch($request);

header('Content-Type: application/json');
echo json_encode($response->toArray());

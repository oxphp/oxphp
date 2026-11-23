<?php

declare(strict_types=1);

// The holder for test_curl_shared_client_does_not_reenter_perform: a request that
// adds a transfer of ?delay= ms to the client in $sharedState['curl_client'] and
// drives it with the client's loop, raising $sharedState['curl_in_select'] for the
// length of each curl_multi_select(). Answers JSON: why its transfer failed (null
// when it did not), its body, or what it threw.

require_once __DIR__ . '/curl_shared_client.php';

$peer = getenv('NET_PEER') ?: 'hooks-netpeer';
$delay = (int) ($_GET['delay'] ?? 2500);
$result = [];

try {
    $client = $sharedState['curl_client'];
    $ch = curl_init("http://{$peer}:8080/{$delay}/holder");
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
    $result['added'] = curl_multi_add_handle($client->mh, $ch);
    $result['failed'] = $client->run('holder', $ch, static function (bool $in) use (&$sharedState): void {
        $sharedState['curl_in_select'] = $in;
    });
    $result['body'] = curl_multi_getcontent($ch);
    curl_multi_remove_handle($client->mh, $ch);
} catch (\Throwable $e) {
    $result['threw'] = get_class($e) . ': ' . $e->getMessage();
}

$sharedState['curl_in_select'] = false;
echo json_encode($result);

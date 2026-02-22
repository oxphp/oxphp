<?php

if ($_SERVER['REQUEST_METHOD'] !== 'POST') {
    json_response(405, ['error' => 'POST required']);
    return;
}

$files = [];
if (!empty($_FILES)) {
    foreach ($_FILES as $field => $info) {
        if (is_array($info['name'])) {
            for ($i = 0; $i < count($info['name']); $i++) {
                $files[] = [
                    'field'    => $field,
                    'name'     => $info['name'][$i],
                    'type'     => $info['type'][$i],
                    'size'     => $info['size'][$i],
                    'error'    => $info['error'][$i],
                    'tmp_name' => $info['tmp_name'][$i],
                ];
            }
        } else {
            $files[] = [
                'field'    => $field,
                'name'     => $info['name'],
                'type'     => $info['type'],
                'size'     => $info['size'],
                'error'    => $info['error'],
                'tmp_name' => $info['tmp_name'],
            ];
        }
    }
}

json_response(200, [
    'files_count' => count($files),
    'files'       => $files,
    'post'        => $_POST,
    'comment'     => $_POST['comment'] ?? null,
]);

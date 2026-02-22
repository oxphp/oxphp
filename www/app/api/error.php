<?php

$type = $_GET['type'] ?? 'list';
switch ($type) {
    case 'notice':
        echo @$undefined_var;
        json_response(200, ['triggered' => 'notice', 'note' => 'Check server logs for the PHP notice.']);
        break;
    case 'warning':
        $x = 1 / 0;
        json_response(200, ['triggered' => 'warning', 'note' => 'Division by zero warning in logs.']);
        break;
    case 'exception':
        throw new RuntimeException('Demo exception from /api/error?type=exception');
    case 'fatal':
        call_to_undefined_function_xyz();
        break;
    default:
        json_response(200, [
            'types' => ['notice', 'warning', 'exception', 'fatal'],
            'usage' => '/api/error?type=notice',
            'note'  => 'Each type triggers a different PHP error level. Check server logs for structured error output.',
        ]);
}

<?php

$current = $_COOKIE ?: [];
$cookie_json = json_encode($current, JSON_PRETTY_PRINT);

layout('Cookies', <<<HTML
<div class="grid-2">
    <div class="card">
        <div class="card-header">Set Cookie</div>
        <div class="card-body">
            <form id="cookie-form">
                <label>Name <input type="text" id="ck-name" value="demo" required></label>
                <label>Value <input type="text" id="ck-value" value="hello-oxphp" required></label>
                <button type="submit" class="btn">Set</button>
                <button type="button" class="btn btn-secondary" id="ck-clear">Clear All</button>
            </form>
        </div>
    </div>
    <div class="card">
        <div class="card-header">Current Cookies</div>
        <div class="card-body">
            <pre id="cookie-display">{$cookie_json}</pre>
        </div>
    </div>
</div>
HTML);

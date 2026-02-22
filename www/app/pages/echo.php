<?php

layout('Echo Tool', <<<'HTML'
<div class="card">
    <div class="card-header">
        Send a Request
        <div style="float:right;display:flex;gap:6px;flex-wrap:wrap">
            <button class="btn btn-sm" onclick="echoPreset('get')">GET</button>
            <button class="btn btn-sm" onclick="echoPreset('post-json')">POST JSON</button>
            <button class="btn btn-sm" onclick="echoPreset('post-form')">POST Form</button>
            <button class="btn btn-sm" onclick="echoPreset('query')">QUERY</button>
            <button class="btn btn-sm" onclick="echoPreset('put')">PUT</button>
            <button class="btn btn-sm" onclick="echoPreset('delete')">DELETE</button>
        </div>
    </div>
    <div class="card-body">
        <form id="echo-form">
            <div class="form-row">
                <label>Method
                    <select id="echo-method">
                        <option>GET</option>
                        <option>POST</option>
                        <option>PUT</option>
                        <option>DELETE</option>
                        <option>PATCH</option>
                        <option>QUERY</option>
                    </select>
                </label>
                <label>Path
                    <input type="text" id="echo-path" value="/api/echo?foo=bar&n=42" style="min-width:300px">
                </label>
            </div>
            <div class="form-row">
                <label>Content-Type
                    <select id="echo-ct">
                        <option value="">— none —</option>
                        <option value="application/json" selected>application/json</option>
                        <option value="application/x-www-form-urlencoded">application/x-www-form-urlencoded</option>
                        <option value="text/plain">text/plain</option>
                    </select>
                </label>
            </div>
            <label>Headers <small>(one per line: Name: Value)</small>
                <textarea id="echo-headers" rows="2" placeholder="X-Custom: hello"></textarea>
            </label>
            <label>Body
                <textarea id="echo-body" rows="4" placeholder='{"message": "hello"}'></textarea>
            </label>
            <button type="submit" class="btn">Send Request</button>
        </form>
    </div>
</div>

<div id="echo-result" class="card" style="display:none">
    <div class="card-header">Response <span id="echo-status" class="badge"></span></div>
    <div class="card-body">
        <div id="echo-response-headers" class="mono small"></div>
        <pre id="echo-response-body"></pre>
    </div>
</div>

<script>
function echoPreset(name) {
    const m = document.getElementById('echo-method');
    const p = document.getElementById('echo-path');
    const ct = document.getElementById('echo-ct');
    const h = document.getElementById('echo-headers');
    const b = document.getElementById('echo-body');

    const presets = {
        'get': {
            method: 'GET',
            path: '/api/echo?search=hello&page=1&limit=20',
            ct: '',
            headers: 'Accept: application/json',
            body: ''
        },
        'post-json': {
            method: 'POST',
            path: '/api/echo',
            ct: 'application/json',
            headers: 'X-Request-Source: echo-tool',
            body: JSON.stringify({user: {name: "Alice", email: "alice@example.com"}, action: "create", tags: ["admin", "active"]}, null, 2)
        },
        'post-form': {
            method: 'POST',
            path: '/api/echo',
            ct: 'application/x-www-form-urlencoded',
            headers: '',
            body: 'username=alice&password=secret123&remember=true'
        },
        'query': {
            method: 'QUERY',
            path: '/api/search',
            ct: 'application/json',
            headers: '',
            body: JSON.stringify({filter: {status: "active", role: "admin"}, sort: "created_at", order: "desc"}, null, 2)
        },
        'put': {
            method: 'PUT',
            path: '/api/echo?id=42',
            ct: 'application/json',
            headers: 'X-Request-Source: echo-tool\nIf-Match: "abc123"',
            body: JSON.stringify({name: "Alice Updated", email: "alice.new@example.com", verified: true}, null, 2)
        },
        'delete': {
            method: 'DELETE',
            path: '/api/echo?id=42',
            ct: '',
            headers: 'X-Reason: user-requested',
            body: ''
        }
    };

    const preset = presets[name];
    if (!preset) return;

    m.value = preset.method;
    p.value = preset.path;
    ct.value = preset.ct;
    h.value = preset.headers;
    b.value = preset.body;
}
</script>
HTML);

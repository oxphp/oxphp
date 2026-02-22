<?php

$content = <<<'HTML'
<div class="card">
    <div class="card-header">
        Controls
        <div style="float:right">
            <span id="sse-status" class="badge">idle</span>
        </div>
    </div>
    <div class="card-body">
        <div class="form-row">
            <label>Mode
                <select id="sse-mode">
                    <option value="oxphp">oxphp_stream_flush()</option>
                    <option value="native">Native flush()</option>
                </select>
            </label>
            <label>Events
                <input type="number" id="sse-count" value="10" min="1" max="100">
            </label>
            <label>Delay (ms)
                <input type="number" id="sse-delay" value="1000" min="100" max="5000" step="100">
            </label>
        </div>
        <button id="sse-start" class="btn">Start Stream</button>
        <button id="sse-stop" class="btn btn-secondary" disabled>Stop</button>
    </div>
</div>

<div class="card">
    <div class="card-header">Event Log</div>
    <div class="card-body">
        <div id="sse-progress" style="display:none">
            <div class="progress-bar"><div class="progress-fill" id="sse-fill" style="width:0%"></div></div>
            <div class="small mono" style="color:var(--color-muted);margin-bottom:12px"><span id="sse-counter">0</span> / <span id="sse-total">0</span> events</div>
        </div>
        <pre id="sse-log" style="max-height:400px;overflow-y:auto"></pre>
    </div>
</div>

<div class="card">
    <div class="card-header">How It Works</div>
    <div class="card-body">
        <table class="table-kv">
            <tr>
                <td><code>oxphp_stream_flush()</code></td>
                <td>Activates streaming mode, flushes PHP output buffers, and sends the chunk. Works with any Content-Type.</td>
            </tr>
            <tr>
                <td>Native <code>flush()</code></td>
                <td>Uses only standard PHP: <code>header()</code> + <code>ob_end_flush()</code> + <code>echo</code> + <code>flush()</code>. Streaming auto-activates when <code>Content-Type: text/event-stream</code> is detected by OxPHP.</td>
            </tr>
        </table>
    </div>
</div>

<script>
(function() {
    let source = null;
    const log     = document.getElementById('sse-log');
    const status  = document.getElementById('sse-status');
    const start   = document.getElementById('sse-start');
    const stop    = document.getElementById('sse-stop');
    const mode    = document.getElementById('sse-mode');
    const progress = document.getElementById('sse-progress');
    const fill    = document.getElementById('sse-fill');
    const counter = document.getElementById('sse-counter');
    const total   = document.getElementById('sse-total');

    function appendLog(msg, color) {
        const ts = new Date().toLocaleTimeString();
        log.textContent += `[${ts}] ${msg}\n`;
        log.scrollTop = log.scrollHeight;
    }

    function setStatus(text, bg) {
        status.textContent = text;
        status.style.background = bg || '';
        status.style.color = '#fff';
    }

    start.addEventListener('click', function() {
        if (source) source.close();
        log.textContent = '';

        const count = document.getElementById('sse-count').value;
        const delay = document.getElementById('sse-delay').value;
        const endpoint = mode.value === 'native' ? '/api/sse-native' : '/api/sse';
        const url = `${endpoint}?count=${count}&delay=${delay}`;

        total.textContent = count;
        counter.textContent = '0';
        fill.style.width = '0%';
        progress.style.display = '';

        appendLog(`[${mode.value}] Connecting to ${url}...`);
        setStatus('connecting', 'var(--color-muted)');
        start.disabled = true;
        stop.disabled = false;

        source = new EventSource(url);

        source.onopen = function() {
            setStatus('streaming', 'var(--color-accent)');
            appendLog('Connected — receiving events');
        };

        source.onmessage = function(e) {
            const data = JSON.parse(e.data);
            const tag = data.mode === 'native' ? ' [native]' : '';
            counter.textContent = data.counter;
            fill.style.width = `${(data.counter / data.total) * 100}%`;
            appendLog(`Event #${data.counter}/${data.total} — ${data.time} (worker ${data.worker})${tag}`);
        };

        source.addEventListener('done', function() {
            fill.style.width = '100%';
            appendLog('Stream complete.');
            cleanup();
        });

        source.onerror = function() {
            if (source.readyState === EventSource.CLOSED) {
                appendLog('Connection closed.');
            } else {
                appendLog('Connection error.');
            }
            cleanup();
        };
    });

    stop.addEventListener('click', function() {
        if (source) {
            source.close();
            appendLog('Stopped by user.');
        }
        cleanup();
    });

    function cleanup() {
        if (source) { source.close(); source = null; }
        setStatus('idle', '');
        status.style.color = '';
        status.style.background = '';
        start.disabled = false;
        stop.disabled = true;
    }
})();
</script>
HTML;

layout('SSE Streaming', $content);

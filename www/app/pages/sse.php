<?php

$content = <<<'HTML'
<div class="card">
    <h2>Server-Sent Events</h2>
    <p>Real-time streaming from PHP to the browser using <code>EventSource</code>.</p>

    <div style="display:flex;gap:1rem;margin:1rem 0;align-items:center;flex-wrap:wrap">
        <label>Events: <input type="number" id="sse-count" value="10" min="1" max="100" style="width:60px"></label>
        <label>Delay (ms): <input type="number" id="sse-delay" value="1000" min="100" max="5000" step="100" style="width:80px"></label>
        <select id="sse-mode" style="padding:0.3rem 0.5rem;border-radius:4px;border:1px solid var(--border)">
            <option value="oxphp">oxphp_stream_flush()</option>
            <option value="native">Native flush()</option>
        </select>
        <button id="sse-start" class="btn">Start Stream</button>
        <button id="sse-stop" class="btn" disabled>Stop</button>
        <span id="sse-status" class="mono" style="color:var(--muted)">idle</span>
    </div>

    <div style="margin-bottom:1rem">
        <details>
            <summary style="cursor:pointer;color:var(--muted)">Implementation details</summary>
            <div style="margin-top:0.5rem;font-size:0.9rem;line-height:1.6">
                <p><strong>oxphp_stream_flush()</strong> — activates streaming, flushes PHP output buffers, and sends the chunk. Works with any Content-Type.</p>
                <p><strong>Native flush()</strong> — uses only standard PHP: <code>header()</code> + <code>ob_end_flush()</code> + <code>echo</code> + <code>flush()</code>. Streaming auto-activates when <code>Content-Type: text/event-stream</code> is detected.</p>
            </div>
        </details>
    </div>

    <div id="sse-log" class="mono" style="background:var(--bg-code);padding:1rem;border-radius:6px;max-height:400px;overflow-y:auto;font-size:0.85rem;line-height:1.6"></div>
</div>

<script>
(function() {
    let source = null;
    const log    = document.getElementById('sse-log');
    const status = document.getElementById('sse-status');
    const start  = document.getElementById('sse-start');
    const stop   = document.getElementById('sse-stop');
    const mode   = document.getElementById('sse-mode');

    function appendLog(msg, cls) {
        const line = document.createElement('div');
        line.textContent = msg;
        if (cls) line.style.color = cls;
        log.appendChild(line);
        log.scrollTop = log.scrollHeight;
    }

    start.addEventListener('click', function() {
        if (source) source.close();
        log.innerHTML = '';

        const count = document.getElementById('sse-count').value;
        const delay = document.getElementById('sse-delay').value;
        const endpoint = mode.value === 'native' ? '/api/sse-native' : '/api/sse';
        const url = `${endpoint}?count=${count}&delay=${delay}`;

        appendLog(`[${mode.value}] Connecting to ${url}...`, 'var(--muted)');
        status.textContent = 'connecting...';
        start.disabled = true;
        stop.disabled = false;

        source = new EventSource(url);

        source.onopen = function() {
            status.textContent = 'connected';
            status.style.color = 'var(--green, #2a9d2a)';
            appendLog('Connected — receiving events', 'var(--green, #2a9d2a)');
        };

        source.onmessage = function(e) {
            const data = JSON.parse(e.data);
            const tag = data.mode === 'native' ? ' [native]' : '';
            appendLog(`#${data.counter}/${data.total} — ${data.time} (worker ${data.worker})${tag}`);
        };

        source.addEventListener('done', function() {
            appendLog('Stream complete.', 'var(--muted)');
            cleanup();
        });

        source.onerror = function() {
            if (source.readyState === EventSource.CLOSED) {
                appendLog('Connection closed.', 'var(--muted)');
            } else {
                appendLog('Connection error.', 'var(--red, #c00)');
            }
            cleanup();
        };
    });

    stop.addEventListener('click', function() {
        if (source) {
            source.close();
            appendLog('Stopped by user.', 'var(--muted)');
        }
        cleanup();
    });

    function cleanup() {
        if (source) { source.close(); source = null; }
        status.textContent = 'idle';
        status.style.color = 'var(--muted)';
        start.disabled = false;
        stop.disabled = true;
    }
})();
</script>
HTML;

layout('SSE Streaming', $content);

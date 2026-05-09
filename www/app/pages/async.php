<?php

$content = <<<'HTML'
<div class="card">
    <div class="card-header">
        Controls
        <div style="float:right">
            <span id="async-status" class="badge">idle</span>
        </div>
    </div>
    <div class="card-body">
        <div style="display:flex;gap:8px;flex-wrap:wrap">
            <button id="btn-parallel" class="btn">Parallel (4 tasks)</button>
            <button id="btn-race" class="btn">Race (3 tasks)</button>
            <button id="btn-compute" class="btn">Compute (fan-out)</button>
        </div>
    </div>
</div>

<div class="grid-2">
    <div class="card">
        <div class="card-header">Results</div>
        <div class="card-body">
            <pre id="async-output" style="max-height:400px;overflow-y:auto;min-height:120px">Click a button above to run an async demo.</pre>
        </div>
    </div>

    <div class="card">
        <div class="card-header">Timing</div>
        <div class="card-body">
            <div id="async-chart" style="min-height:120px">
                <p class="small" style="color:var(--color-muted)">Timing visualization will appear here after running a demo.</p>
            </div>
        </div>
    </div>
</div>

<div class="card">
    <div class="card-header">How It Works</div>
    <div class="card-body">
        <table class="table-kv">
            <tr>
                <td><code>oxphp_async()</code></td>
                <td>Dispatches a closure to run on an async worker thread. Returns a promise handle immediately without blocking.</td>
            </tr>
            <tr>
                <td><code>oxphp_async_await()</code></td>
                <td>Blocks until a single promise resolves and returns its result.</td>
            </tr>
            <tr>
                <td><code>oxphp_async_await_all()</code></td>
                <td>Blocks until <strong>all</strong> promises resolve. Returns an array of results in the same order as the input promises.</td>
            </tr>
            <tr>
                <td><code>oxphp_async_await_race()</code></td>
                <td>Blocks until the <strong>first</strong> promise resolves. Returns the winner's result — useful for racing tasks or timeouts.</td>
            </tr>
            <tr>
                <td><code>ASYNC_WORKERS</code></td>
                <td>Set in <span class="mono">compose.yml</span> to control the number of async worker threads in the pool (e.g. <code>ASYNC_WORKERS=4</code>).</td>
            </tr>
        </table>
    </div>
</div>

<style>
    .chart-row {
        display: flex;
        align-items: center;
        margin-bottom: 8px;
        font-size: 13px;
        font-family: var(--mono);
    }
    .chart-label {
        width: 80px;
        flex-shrink: 0;
        color: var(--color-muted);
    }
    .chart-bar-wrap {
        flex: 1;
        height: 24px;
        background: var(--color-bg);
        border-radius: 4px;
        overflow: hidden;
        position: relative;
    }
    .chart-bar {
        height: 100%;
        border-radius: 4px;
        transition: width 0.4s ease-out;
        display: flex;
        align-items: center;
        padding-left: 8px;
        font-size: 11px;
        color: #fff;
        white-space: nowrap;
    }
    .chart-bar.task { background: var(--color-success); }
    .chart-bar.wall { background: var(--color-warning); }
    .chart-bar.winner { background: var(--color-accent); box-shadow: 0 0 8px rgba(119,123,180,0.4); }
    .chart-bar.seq { background: var(--color-border); }
    .chart-legend {
        display: flex;
        gap: 16px;
        margin-top: 12px;
        font-size: 11px;
        color: var(--color-muted);
    }
    .chart-legend-dot {
        display: inline-block;
        width: 10px;
        height: 10px;
        border-radius: 2px;
        margin-right: 4px;
        vertical-align: middle;
    }
</style>

<script>
(function() {
    var output   = document.getElementById('async-output');
    var chart    = document.getElementById('async-chart');
    var status   = document.getElementById('async-status');
    var btnPar   = document.getElementById('btn-parallel');
    var btnRace  = document.getElementById('btn-race');
    var btnComp  = document.getElementById('btn-compute');

    function setStatus(text, bg) {
        status.textContent = text;
        status.style.background = bg || '';
        status.style.color = bg ? '#fff' : '';
    }

    function setButtons(disabled) {
        btnPar.disabled = disabled;
        btnRace.disabled = disabled;
        btnComp.disabled = disabled;
    }

    function buildBar(label, widthPct, cls, text) {
        var row = document.createElement('div');
        row.className = 'chart-row';

        var lbl = document.createElement('span');
        lbl.className = 'chart-label';
        lbl.textContent = label;

        var wrap = document.createElement('div');
        wrap.className = 'chart-bar-wrap';

        var bar = document.createElement('div');
        bar.className = 'chart-bar ' + cls;
        bar.style.width = widthPct + '%';
        bar.textContent = text;

        wrap.appendChild(bar);
        row.appendChild(lbl);
        row.appendChild(wrap);
        return row;
    }

    function buildLegend(items) {
        var legend = document.createElement('div');
        legend.className = 'chart-legend';
        for (var i = 0; i < items.length; i++) {
            var span = document.createElement('span');
            if (items[i].color) {
                var dot = document.createElement('span');
                dot.className = 'chart-legend-dot';
                dot.style.background = items[i].color;
                span.appendChild(dot);
            }
            if (items[i].mono) {
                span.style.fontFamily = 'var(--mono)';
            }
            span.appendChild(document.createTextNode(items[i].text));
            legend.appendChild(span);
        }
        return legend;
    }

    function renderChart(data) {
        chart.textContent = '';

        if (data.mode === 'parallel') {
            var maxMs = Math.max(data.sequential_ms, data.wall_ms, 1);
            var results = data.results || {};
            var tasks = Array.isArray(results) ? results : Object.values(results);
            for (var i = 0; i < tasks.length; i++) {
                var t = tasks[i];
                var ms = t.actual_ms || t.sleep_ms;
                var pct = (ms / maxMs * 100).toFixed(1);
                chart.appendChild(buildBar('Task ' + (t.task || (i + 1)), pct, 'task', ms + ' ms'));
            }
            chart.appendChild(buildBar('Wall', (data.wall_ms / maxMs * 100).toFixed(1), 'wall', data.wall_ms + ' ms'));
            chart.appendChild(buildBar('Sequential', (data.sequential_ms / maxMs * 100).toFixed(1), 'seq', data.sequential_ms + ' ms'));
            chart.appendChild(buildLegend([
                { color: 'var(--color-success)', text: 'Task duration' },
                { color: 'var(--color-warning)', text: 'Wall time (' + data.speedup + ' speedup)' },
                { color: 'var(--color-border)', text: 'If sequential' },
            ]));
        } else if (data.mode === 'race') {
            var durations = data.durations || [];
            var maxMs = Math.max.apply(null, durations.concat([data.wall_ms, 1]));
            var winnerTask = data.winner && data.winner.value ? data.winner.value.task : -1;
            for (var i = 0; i < durations.length; i++) {
                var isWinner = (i + 1) === winnerTask;
                var pct = (durations[i] / maxMs * 100).toFixed(1);
                var cls = isWinner ? 'winner' : 'task';
                var text = durations[i] + ' ms' + (isWinner ? ' (winner)' : '');
                chart.appendChild(buildBar('Task ' + (i + 1), pct, cls, text));
            }
            chart.appendChild(buildBar('Wall', (data.wall_ms / maxMs * 100).toFixed(1), 'wall', data.wall_ms + ' ms'));
            chart.appendChild(buildLegend([
                { color: 'var(--color-accent)', text: 'Winner' },
                { color: 'var(--color-success)', text: 'Other tasks' },
                { color: 'var(--color-warning)', text: 'Wall time' },
            ]));
        } else if (data.mode === 'compute') {
            var raw = data.results || {};
            var tasks = Array.isArray(raw) ? raw : Object.values(raw);
            var maxMs = 1;
            for (var i = 0; i < tasks.length; i++) {
                if (tasks[i].actual_ms > maxMs) maxMs = tasks[i].actual_ms;
            }
            if (data.wall_ms > maxMs) maxMs = data.wall_ms;
            for (var i = 0; i < tasks.length; i++) {
                var t = tasks[i];
                var pct = Math.max((t.actual_ms / maxMs * 100).toFixed(1), 3);
                chart.appendChild(buildBar('Chunk ' + t.chunk, pct, 'task', t.actual_ms + ' ms (' + t.size + ' items)'));
            }
            var wallPct = Math.max((data.wall_ms / maxMs * 100).toFixed(1), 3);
            chart.appendChild(buildBar('Wall', wallPct, 'wall', data.wall_ms + ' ms'));
            chart.appendChild(buildLegend([
                { color: 'var(--color-success)', text: 'Chunk compute time' },
                { color: 'var(--color-warning)', text: 'Wall time' },
                { mono: true, text: 'Total: ' + data.total.toLocaleString() },
            ]));
        } else {
            var p = document.createElement('p');
            p.className = 'small';
            p.style.color = 'var(--color-muted)';
            p.textContent = 'No chart data.';
            chart.appendChild(p);
        }
    }

    function runMode(mode) {
        setStatus('running', 'var(--color-accent)');
        setButtons(true);
        output.textContent = 'Dispatching async tasks...';
        chart.textContent = '';
        var waiting = document.createElement('p');
        waiting.className = 'small';
        waiting.style.color = 'var(--color-muted)';
        waiting.textContent = 'Waiting for results...';
        chart.appendChild(waiting);

        fetch('/api/async?mode=' + encodeURIComponent(mode))
            .then(function(res) { return res.json(); })
            .then(function(data) {
                output.textContent = JSON.stringify(data, null, 2);
                renderChart(data);
                setStatus('done', 'var(--color-ox)');
                setButtons(false);
                setTimeout(function() { setStatus('idle', ''); }, 2000);
            })
            .catch(function(err) {
                output.textContent = 'Error: ' + err.message;
                setStatus('error', 'var(--color-ox)');
                setButtons(false);
            });
    }

    btnPar.addEventListener('click', function() { runMode('parallel'); });
    btnRace.addEventListener('click', function() { runMode('race'); });
    btnComp.addEventListener('click', function() { runMode('compute'); });
})();
</script>
HTML;

layout('Async Promises', $content);

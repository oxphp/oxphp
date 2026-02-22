document.addEventListener('DOMContentLoaded', () => {

    // ── Echo Tool ────────────────────────────────────
    const echoForm = document.getElementById('echo-form');
    if (echoForm) {
        echoForm.addEventListener('submit', async (e) => {
            e.preventDefault();
            const method = document.getElementById('echo-method').value;
            const path   = document.getElementById('echo-path').value;
            const ct     = document.getElementById('echo-ct').value;
            const body   = document.getElementById('echo-body').value;
            const rawH   = document.getElementById('echo-headers').value;

            const headers = {};
            if (ct) headers['Content-Type'] = ct;
            for (const line of rawH.split('\n')) {
                const idx = line.indexOf(':');
                if (idx > 0) {
                    headers[line.slice(0, idx).trim()] = line.slice(idx + 1).trim();
                }
            }

            const opts = { method, headers };
            if (body && method !== 'GET' && method !== 'HEAD') {
                opts.body = body;
            }

            try {
                const resp = await fetch(path, opts);
                showEchoResult(resp);
            } catch (err) {
                showEchoError(err.message);
            }
        });
    }

    async function showEchoResult(resp) {
        const result = document.getElementById('echo-result');
        const status = document.getElementById('echo-status');
        const hdrs   = document.getElementById('echo-response-headers');
        const body   = document.getElementById('echo-response-body');

        result.style.display = 'block';

        status.textContent = `${resp.status} ${resp.statusText}`;
        status.className = 'badge ' + (resp.ok ? 'method' : '');
        status.style.background = resp.ok ? '' : 'rgba(183, 71, 42, 0.15)';
        status.style.color = resp.ok ? '' : '#B7472A';

        const headerLines = [];
        resp.headers.forEach((val, key) => {
            headerLines.push(`${key}: ${val}`);
        });
        hdrs.textContent = headerLines.join('\n');

        const text = await resp.text();
        try {
            body.textContent = JSON.stringify(JSON.parse(text), null, 2);
        } catch {
            body.textContent = text;
        }
    }

    function showEchoError(msg) {
        const result = document.getElementById('echo-result');
        const status = document.getElementById('echo-status');
        const body   = document.getElementById('echo-response-body');
        const hdrs   = document.getElementById('echo-response-headers');

        result.style.display = 'block';
        status.textContent = 'Error';
        hdrs.textContent = '';
        body.textContent = msg;
    }

    // ── File Upload ──────────────────────────────────
    const uploadForm = document.getElementById('upload-form');
    if (uploadForm) {
        uploadForm.addEventListener('submit', async (e) => {
            e.preventDefault();
            const formData = new FormData();
            const files = document.getElementById('upload-files').files;
            for (const f of files) {
                formData.append('files[]', f);
            }
            const comment = document.getElementById('upload-comment').value;
            if (comment) formData.append('comment', comment);

            try {
                const resp = await fetch('/api/upload', { method: 'POST', body: formData });
                const data = await resp.json();
                document.getElementById('upload-result').style.display = 'block';
                document.getElementById('upload-response').textContent = JSON.stringify(data, null, 2);
            } catch (err) {
                document.getElementById('upload-result').style.display = 'block';
                document.getElementById('upload-response').textContent = 'Error: ' + err.message;
            }
        });
    }

    // ── Cookies ──────────────────────────────────────
    const cookieForm = document.getElementById('cookie-form');
    if (cookieForm) {
        cookieForm.addEventListener('submit', async (e) => {
            e.preventDefault();
            const name  = document.getElementById('ck-name').value;
            const value = document.getElementById('ck-value').value;

            await fetch(`/api/cookies/set?name=${encodeURIComponent(name)}&value=${encodeURIComponent(value)}`);
            refreshCookies();
        });

        document.getElementById('ck-clear')?.addEventListener('click', async () => {
            await fetch('/api/cookies/clear');
            refreshCookies();
        });
    }

    async function refreshCookies() {
        const resp = await fetch('/api/cookies');
        const data = await resp.json();
        const display = document.getElementById('cookie-display');
        if (display) {
            display.textContent = JSON.stringify(data.cookies, null, 2);
        }
    }
});

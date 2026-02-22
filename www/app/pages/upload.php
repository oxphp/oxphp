<?php

layout('File Upload', <<<'HTML'
<div class="card">
    <div class="card-header">Upload Files</div>
    <div class="card-body">
        <form id="upload-form" enctype="multipart/form-data">
            <label>Select files
                <input type="file" id="upload-files" name="files[]" multiple>
            </label>
            <label>Comment
                <input type="text" id="upload-comment" name="comment" placeholder="Optional comment">
            </label>
            <button type="submit" class="btn">Upload</button>
        </form>
    </div>
</div>
<div id="upload-result" class="card" style="display:none">
    <div class="card-header">Result</div>
    <div class="card-body"><pre id="upload-response"></pre></div>
</div>
HTML);

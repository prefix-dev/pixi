if ($env:PIXI_LOCK_FILE -and (Test-Path $env:PIXI_LOCK_FILE -PathType Leaf)) {
    $global:_pixi_last_lock_time = (Get-Item $env:PIXI_LOCK_FILE).LastWriteTime
} elseif ($env:PIXI_WORKSPACE_ROOT -and (Test-Path (Join-Path $env:PIXI_WORKSPACE_ROOT "pixi.lock") -PathType Leaf)) {
    $global:_pixi_last_lock_time = (Get-Item (Join-Path $env:PIXI_WORKSPACE_ROOT "pixi.lock")).LastWriteTime
} elseif ($env:PIXI_PROJECT_ROOT -and (Test-Path (Join-Path $env:PIXI_PROJECT_ROOT "pixi.lock") -PathType Leaf)) {
    $global:_pixi_last_lock_time = (Get-Item (Join-Path $env:PIXI_PROJECT_ROOT "pixi.lock")).LastWriteTime
}

function global:__pixi_watch_lock {
    $lock_file = $env:PIXI_LOCK_FILE
    if (-not $lock_file -or -not (Test-Path $lock_file -PathType Leaf)) {
        $root = if ($env:PIXI_WORKSPACE_ROOT) { $env:PIXI_WORKSPACE_ROOT } else { $env:PIXI_PROJECT_ROOT }
        if ($root) {
            $lock_file = Join-Path $root "pixi.lock"
        }
    }

    if ($lock_file -and (Test-Path $lock_file -PathType Leaf)) {
        $current_time = (Get-Item $lock_file).LastWriteTime
        if ($global:_pixi_last_lock_time -and $current_time -gt $global:_pixi_last_lock_time) {
            $global:_pixi_last_lock_time = $current_time
            & $env:PIXI_EXE shell-hook --change-ps1 false --shell powershell | Invoke-Expression
        }
    }
}

function global:pixi {
    $first_arg = $args[0]
    & $env:PIXI_EXE @args
    if ($LASTEXITCODE -ne 0) {
        return
    }

    switch -regex ($first_arg) {
        '^(add|a|remove|rm|install|i|update|u)$' {
            $lock_file = $env:PIXI_LOCK_FILE
            if (-not $lock_file -or -not (Test-Path $lock_file -PathType Leaf)) {
                $root = if ($env:PIXI_WORKSPACE_ROOT) { $env:PIXI_WORKSPACE_ROOT } else { $env:PIXI_PROJECT_ROOT }
                if ($root) {
                    $lock_file = Join-Path $root "pixi.lock"
                }
            }
            if ($lock_file -and (Test-Path $lock_file -PathType Leaf)) {
                $global:_pixi_last_lock_time = (Get-Item $lock_file).LastWriteTime
            }
            & $env:PIXI_EXE shell-hook --change-ps1 false --shell powershell | Invoke-Expression
        }
    }
}

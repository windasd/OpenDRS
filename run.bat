@echo off
chcp 65001 >nul
set "HERE=%~dp0"
set "PYTHONPATH=%HERE%src"
set "PYTHONUTF8=1"
"%HERE%.venv\Scripts\pythonw.exe" -m opendrs %*

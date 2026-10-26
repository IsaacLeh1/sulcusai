@echo off
rem yue2.cpp for Windows on the processor (every CPU variant as a loadable
rem backend, no OpenMP, static VC++ runtime), like the other engines.
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
if exist C:\dev\yc rd /s /q C:\dev\yc
cmake -S C:\dev\tools\music\yue2.cpp -B C:\dev\yc -G Ninja -DCMAKE_BUILD_TYPE=Release -DGGML_CPU_ALL_VARIANTS=ON -DGGML_BACKEND_DL=ON -DGGML_OPENMP=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded -DCMAKE_POLICY_DEFAULT_CMP0091=NEW || exit /b 1
cmake --build C:\dev\yc -j 6 || exit /b 1

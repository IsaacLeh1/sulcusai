@echo off
rem qwentts.cpp for Windows on the processor, built like the acestep engine:
rem every CPU variant as a loadable backend, no OpenMP, static VC++ runtime.
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
cd /d C:\dev\tools\voice\qwentts.cpp
if exist build-cpu rd /s /q build-cpu
mkdir build-cpu
cd build-cpu
cmake .. -G Ninja -DCMAKE_BUILD_TYPE=Release -DGGML_CPU_ALL_VARIANTS=ON -DGGML_BACKEND_DL=ON -DGGML_OPENMP=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded -DCMAKE_POLICY_DEFAULT_CMP0091=NEW || exit /b 1
cmake --build . -j 6 || exit /b 1

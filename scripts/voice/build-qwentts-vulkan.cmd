@echo off
rem qwentts.cpp for Windows with ggml's Vulkan backend, built like the acestep
rem Vulkan engine: glslc from Google's shaderc CI build, the Vulkan and SPIR-V
rem headers at vulkan-sdk-1.4.363.0, vulkan-1.lib made from the driver's DLL,
rem and a short build path (the shader generator's paths pass 260 characters).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set VK=C:\dev\vk
if not exist %VK%\vulkan-1.lib (
  dumpbin /nologo /exports C:\Windows\System32\vulkan-1.dll > %VK%\exports.txt || exit /b 1
  py -3.12 -I %~dp0def_from_dumpbin.py %VK%\exports.txt %VK%\vulkan-1.def || exit /b 1
  lib /nologo /def:%VK%\vulkan-1.def /machine:x64 /out:%VK%\vulkan-1.lib || exit /b 1
)
if not exist %VK%\spirv\share (
  cmake -S %VK%\SPIRV-Headers-vulkan-sdk-1.4.363.0 -B %VK%\spirv-build -G Ninja -DCMAKE_INSTALL_PREFIX=%VK%\spirv -DSPIRV_HEADERS_ENABLE_TESTS=OFF || exit /b 1
  cmake --install %VK%\spirv-build || exit /b 1
)
if exist C:\dev\qb rd /s /q C:\dev\qb
cmake -S C:\dev\tools\voice\qwentts.cpp -B C:\dev\qb -G Ninja -DCMAKE_BUILD_TYPE=Release -DGGML_CPU_ALL_VARIANTS=ON -DGGML_BACKEND_DL=ON -DGGML_OPENMP=OFF -DGGML_VULKAN=ON -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded -DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DVulkan_INCLUDE_DIR=%VK%\Vulkan-Headers-vulkan-sdk-1.4.363.0\include -DVulkan_LIBRARY=%VK%\vulkan-1.lib -DVulkan_GLSLC_EXECUTABLE=%VK%\shaderc\install\bin\glslc.exe -DCMAKE_PREFIX_PATH=%VK%\spirv "-DCMAKE_CXX_FLAGS=/I%VK%\spirv\include" || exit /b 1
cmake --build C:\dev\qb -j 6 || exit /b 1

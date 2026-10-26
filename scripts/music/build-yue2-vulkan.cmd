@echo off
rem yue2.cpp for Windows with ggml's Vulkan backend (and every CPU variant),
rem built like the acestep and qwentts engines: the Vulkan and SPIR-V
rem headers and glslc under C:\dev\vk (see scripts\voice\README.md), a static
rem VC++ runtime and a short build path.
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set VK=C:\dev\vk
if exist C:\dev\yb rd /s /q C:\dev\yb
cmake -S C:\dev\tools\music\yue2.cpp -B C:\dev\yb -G Ninja -DCMAKE_BUILD_TYPE=Release -DGGML_CPU_ALL_VARIANTS=ON -DGGML_BACKEND_DL=ON -DGGML_OPENMP=OFF -DGGML_VULKAN=ON -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded -DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DVulkan_INCLUDE_DIR=%VK%\Vulkan-Headers-vulkan-sdk-1.4.363.0\include -DVulkan_LIBRARY=%VK%\vulkan-1.lib -DVulkan_GLSLC_EXECUTABLE=%VK%\shaderc\install\bin\glslc.exe -DCMAKE_PREFIX_PATH=%VK%\spirv "-DCMAKE_CXX_FLAGS=/I%VK%\spirv\include" || exit /b 1
cmake --build C:\dev\yb -j 6 || exit /b 1

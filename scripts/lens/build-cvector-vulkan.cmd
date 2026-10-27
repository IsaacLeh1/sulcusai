@echo off
rem llama.cpp's control-vector maker (llama-cvector-generator) for the Lens's
rem steering, at the same build as the chat engine (catalog engine.build),
rem as one program with ggml's Vulkan backend inside: the engine download
rem doesn't include it, and its shared libraries come from a different
rem compiler, so nothing of theirs is reused. The Vulkan and SPIR-V headers
rem and glslc under C:\dev\vk (see scripts\voice\README.md), a static VC++
rem runtime and a short build path, like the other engines.
rem   git clone --depth 1 --branch b11450 https://github.com/ggml-org/llama.cpp C:\dev\tools\llama.cpp-b11450
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set VK=C:\dev\vk
set SRC=C:\dev\tools\llama.cpp-b11450
if exist C:\dev\cvb rd /s /q C:\dev\cvb
cmake -S %SRC% -B C:\dev\cvb -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_BACKEND_DL=OFF -DGGML_NATIVE=OFF -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_OPENMP=OFF -DGGML_VULKAN=ON -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_TOOLS=ON -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded -DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DVulkan_INCLUDE_DIR=%VK%\Vulkan-Headers-vulkan-sdk-1.4.363.0\include -DVulkan_LIBRARY=%VK%\vulkan-1.lib -DVulkan_GLSLC_EXECUTABLE=%VK%\shaderc\install\bin\glslc.exe -DCMAKE_PREFIX_PATH=%VK%\spirv "-DCMAKE_CXX_FLAGS=/I%VK%\spirv\include" || exit /b 1
cmake --build C:\dev\cvb -j 6 --target llama-cvector-generator || exit /b 1
dir C:\dev\cvb\bin\llama-cvector-generator.exe

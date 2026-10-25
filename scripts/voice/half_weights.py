# SPDX-License-Identifier: AGPL-3.0-only
"""Stores an ONNX model's large float32 weights as float16, each with a Cast
back to float32 (computed in float32 as before; ONNX Runtime folds the casts
when it loads). Halves the download. Usage:
    python -I half_weights.py in.onnx out.onnx"""
import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper, TensorProto

src, dst = sys.argv[1], sys.argv[2]
m = onnx.load(src)
g = m.graph
casts = []
kept = []
for init in g.initializer:
    if init.data_type == TensorProto.FLOAT and np.prod(init.dims) >= 1024:
        arr = numpy_helper.to_array(init)
        if np.abs(arr).max() < 65000:
            half = numpy_helper.from_array(arr.astype(np.float16), init.name + "__f16")
            kept.append(half)
            casts.append(helper.make_node("Cast", [init.name + "__f16"], [init.name], to=TensorProto.FLOAT, name=init.name + "__cast"))
            continue
    kept.append(init)
del g.initializer[:]
g.initializer.extend(kept)
nodes = list(g.node)
del g.node[:]
g.node.extend(casts + nodes)
onnx.save(m, dst, save_as_external_data=False) if m.ByteSize() < 2_000_000_000 else onnx.save(m, dst, save_as_external_data=True)
print(src, "->", dst, len(casts), "weights halved")

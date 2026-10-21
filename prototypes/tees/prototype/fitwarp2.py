import os, sys
sys.argv = ['x']
HERE = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(HERE, 'fitwarp.py'), encoding='utf-8').read().split("x0 = np.zeros(5)")[0]
exec(compile(src, 'fitwarp_head', 'exec'))
res = []
for b0 in (-80, -70, -60, -50, -40):
    for b1 in (-0.1, -0.05, 0.0, 0.05):
        for a1 in (-0.05, 0.0, 0.05):
            res.append((loss(np.array([0, a1, b0, b1, 0.0])), b0, b1, a1))
res.sort()
for r in res[:8]: print(r)
np.save(os.path.join(HERE, 'warp.npy'), np.array([0, res[0][3], res[0][1], res[0][2], 0.0]))

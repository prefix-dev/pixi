# Extremely simple example of using pycosat to show we can run sdist packages
import pycosat
cnf = [[1, -5, 4], [-1, 5, 3, 4], [-3, -4]]
result = pycosat.solve(cnf)
print(result)

% /// conda-script
% channels = ["conda-forge"]
% entrypoint = "octave --no-gui --quiet --eval 'pkg rebuild -global' && octave --no-gui --quiet ${SCRIPT}"
%
% [dependencies]
% octave = "*"
% octave-statistics = "*"
% /// end-conda-script
pkg load statistics

points = [0 0; 3 4; 0 4];
distances = pdist(points);
printf("pairwise distances: %s\n", mat2str(distances))
printf("normcdf(0) = %.4f\n", normcdf(0))

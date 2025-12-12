#!/usr/bin/env perl
# /// conda-script
# [dependencies]
# perl = "5.32.*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "perl"
# /// end-conda-script

# A simple Hello World Perl script demonstrating conda-script metadata
# Run with: pixi exec hello_perl.pl

use strict;
use warnings;
use Config;

print "=" x 60 . "\n";
print "Hello from Perl with conda-script!\n";
print "=" x 60 . "\n";
print "Perl version: $]\n";
print "Platform: $^O $Config{archname}\n";
print "=" x 60 . "\n";

# Simple Perl example
my @numbers = (1..10);
my $sum = 0;
$sum += $_ for @numbers;
my $mean = $sum / scalar(@numbers);

printf "Sum of 1 to 10: %d\n", $sum;
printf "Mean of 1 to 10: %.2f\n", $mean;

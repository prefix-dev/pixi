# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "perl ${SCRIPT}"
#
# [dependencies]
# perl = "*"
# perl-uri = "*"
# /// end-conda-script
use strict;
use warnings;
use URI;

my $uri = URI->new("https://prefix.dev/channels/conda-forge?platform=linux-64");
print "scheme = ", $uri->scheme, "\n";
print "host = ", $uri->host, "\n";
print "path = ", $uri->path, "\n";
my %query = $uri->query_form;
print "platform = $query{platform}\n";

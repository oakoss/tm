#!/usr/bin/perl
# Cold-start time of each binary: the median and 90th percentile of 200 runs.
use strict;
use warnings;
use Time::HiRes qw(time);

my ($db, @bins) = @ARGV;
for my $bin (@bins) {
    system("$bin $db >/dev/null") == 0 or die "$bin failed" for 1 .. 10;
    my @t;
    for (1 .. 200) {
        my $s = time;
        system("$bin $db >/dev/null") == 0 or die "$bin failed";
        push @t, (time - $s) * 1000;
    }
    @t = sort { $a <=> $b } @t;
    printf "%s median=%.2fms p90=%.2fms\n", $bin =~ s{.*/}{}r, $t[100], $t[180];
}

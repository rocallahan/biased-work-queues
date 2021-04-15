# Biased Work Queues

Three implementations of the same simple serial-producer/parallel-consumer benchmark.

## TL;DR

In this benchmark, using a [crossbeam](https://docs.rs/crossbeam/0.8.0/crossbeam/) channel, or Rayon, incurs a lot of context switches and task migrations. A simple "biased work queue" implementation reduces those context switches and task migrations by one or two orders of magnitude. In [Pernosco](https://pernos.co) this gave us a *20% improvement in database build throughput*.

<pre>[roc@fedora biased-work-queues]$ perf stat target/release/biased-work-queues crossbeam
...
      19,556.10 msec task-clock                #    7.756 CPUs utilized
           <b>315,292      context-switches</b>          #    0.016 M/sec
               <b>861      cpu-migrations</b>            #    0.044 K/sec
               206      page-faults               #    0.011 K/sec
    60,346,245,829      cycles                    #    3.086 GHz
     4,425,469,283      instructions              #    0.07  insn per cycle
       746,694,172      branches                  #   38.182 M/sec
         8,187,215      branch-misses             #    1.10% of all branches
...
[roc@fedora biased-work-queues]$ perf stat target/release/biased-work-queues rayon
...
         18,259.15 msec task-clock                #    7.782 CPUs utilized
            <b>23,359      context-switches</b>          #    0.001 M/sec
             <b>1,240      cpu-migrations</b>            #    0.068 K/sec
               345      page-faults               #    0.019 K/sec
    56,425,086,217      cycles                    #    3.090 GHz
     2,132,985,920      instructions              #    0.04  insn per cycle
       226,625,218      branches                  #   12.412 M/sec
         1,371,217      branch-misses             #    0.61% of all branches
...
[roc@fedora biased-work-queues]$ perf stat target/release/biased-work-queues crossbeam-biased
...
         17,864.75 msec task-clock                #    7.480 CPUs utilized
             <b>2,148      context-switches</b>          #    0.120 K/sec
                <b>65      cpu-migrations</b>            #    0.004 K/sec
               208      page-faults               #    0.012 K/sec
    55,242,533,817      cycles                    #    3.092 GHz
     1,894,195,362      instructions              #    0.03  insn per cycle
       173,179,781      branches                  #    9.694 M/sec
           472,696      branch-misses             #    0.27% of all branches
...</pre>

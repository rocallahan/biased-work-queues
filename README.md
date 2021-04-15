# Biased Work Queues

Three implementations of the same simple serial-producer/parallel-consumer benchmark.

## TL;DR

In this benchmark, using a [crossbeam](https://docs.rs/crossbeam/0.8.0/crossbeam/) channel as a work queue, or [Rayon](https://docs.rs/rayon/1.5.0/rayon/), incurs a lot of unnecessary context switches and task migrations on Linux. A simple "[biased work queue](https://github.com/rocallahan/biased-work-queues/blob/main/src/biased_work_queue.rs)" implementation reduces those context switches and task migrations by one or two orders of magnitude. In [Pernosco](https://pernos.co) this gave us a *20% improvement in database build throughput*.

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

## The benchmark scenario

A serial producer produces a stream of work items. Those items are processed by a pool of _num-cpus_ parallel consumers. Processing an item takes about _num-cpus_/2 times the CPU time to produce an item, so a single instance of this problem running alone will bottleneck on the producer and use a bit more than half the CPU power in the system. We run three instances simultaneously so the system is fully loaded.

Under these conditions, a naive work queue implementation (and to a lesser extent, the less naive scheduling in Rayon) interacts poorly with the Linux CPU scheduler. The problem is that each thread pool has _num-cpus_ threads, many of which are waiting on the work queue at any given point in time, and every time a work item is added to the queue the kernel will wake a consumer thread (an *arbitrary* thread as far as I know), which will often only be able to process one item before going to sleep again. This waking and sleeping of threads causes a large number of context switches and task migrations.

This scenario may seem contrived – why not just use a single thread pool instead of three identical pools? In practice, though, our pools are all doing different kinds of work, live in different processes and may have different lifetimes. Likewise, in this simple example you could reduce the thread pool size to fit the expected CPU utilization of each pool, but in practice, CPU utilization is unpredictable and variable.

In this benchmark those context switches and migrations are immaterial, but in Pernosco's real-world workload they cause significant degradation of throughput.

## Batching

One simple way to mitigate the problem is with batching. Simply submitting multiple work items in a single batch gives threads more work to do every time they wake up:

<pre>[roc@fedora biased-work-queues]$ perf stat target/release/biased-work-queues --batch-size 20 crossbeam
...
         17,959.40 msec task-clock                #    7.597 CPUs utilized
            <b>10,741      context-switches</b>          #    0.598 K/sec
               <b>102      cpu-migrations</b>            #    0.006 K/sec
               208      page-faults               #    0.012 K/sec
    55,505,508,432      cycles                    #    3.091 GHz
     1,959,257,535      instructions              #    0.04  insn per cycle
       188,752,383      branches                  #   10.510 M/sec
         1,060,105      branch-misses             #    0.56% of all branches
...
[roc@fedora biased-work-queues]$ perf stat target/release/biased-work-queues --batch-size 20 rayon
...
         17,471.71 msec task-clock                #    7.794 CPUs utilized
             <b>2,498      context-switches</b>          #    0.143 K/sec
               <b>116      cpu-migrations</b>            #    0.007 K/sec
               312      page-faults               #    0.018 K/sec
    54,013,335,102      cycles                    #    3.091 GHz
     1,845,301,773      instructions              #    0.03  insn per cycle
       171,023,897      branches                  #    9.789 M/sec
           514,769      branch-misses             #    0.30% of all branches
...</pre>

This certainly helps a lot, but I'm not entirely happy about it:
* There is still significant context switching when we're not using Rayon.
* Batching at submission time increases buffering and latency, which can be a problem.
* That gets much worse if we have many producers.
* Responsibility for solving the problem lands on the producer, when it's really a consumer problem.

## Biased work queues

A slightly more flexible approach is to improve the work queue consumption side: number the threads, and try to keep low-numbered threads busy at the expense of high-numbered threads. I'm calling this a [biased work queue](https://github.com/rocallahan/biased-work-queues/blob/main/src/biased_work_queue.rs).

This approach leaves queue submission entirely unchanged. When a consumer thread needs more work, it pulls as many work items off the queue as it can without sleeping, up to a specified `BATCH_SIZE`. If there aren't any, it must sleep, but only the first thread sleeps on the queue itself. Other threads sleep waiting for the previous thread to signal that it is "overloaded" (i.e. that it was able to pull `BATCH_SIZE` items off the queue without sleeping).

This is a very simple idea with a very simple and efficient implementation, and it works really well. In this benchmark it gives fewer context switches and task migrations than even Rayon with batching. Our Rust implementation is generic and can be applied to any kind of concurrent queue that supports blocking and non-blocking reads.

It doesn't entirely fix the latency/buffering penalty. When the consumer pool is very lightly loaded it does eliminate latency, because the first thread is able to pull and process items one at a time as they are submitted. But in general an item might have to wait for O(`BATCH_SIZE`) other items to be processed before it is processed (assuming the consumer pool as a whole is able to keep up with the producer).

I'm sure we're not the first to invent this technique, but I don't remember reading about it before so I thought it was worth writing up.

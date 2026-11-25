#!/usr/bin/env python3
import argparse
import datetime
import os
import pathlib
import shutil
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKLOADS = {
    'search_matching_1mb_file': 'search',
    'search_missing_1mb_file': 'search',
    'replace_all_rust_10k': 'replace',
    'diff_sparse_rust_10k': 'diff',
    'patch_single_1mb_file': 'patch',
    'run_fast_no_match_100_files': 'tree',
    'run_fast_match_100_files': 'tree',
    'cli_text_apply': 'cli',
    'cli_binding_rename': 'cli',
    'cli_saved_plan': 'cli',
    'cli_interactive_review': 'cli',
}


def select(names, areas, skips):
    unknown = (set(names) | set(skips)) - WORKLOADS.keys()
    unknown_areas = set(areas) - set(WORKLOADS.values())
    if unknown or unknown_areas:
        raise ValueError(f'unknown names or areas: {sorted(unknown | unknown_areas)}')
    selected = [n for n, area in WORKLOADS.items()
                if (not names or n in names) and (not areas or area in areas) and n not in skips]
    if not selected:
        raise ValueError('selection matches no workloads')
    return selected


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=['check', 'list', 'run', 'selftest'])
    parser.add_argument('--tier', choices=['edit', 'change'], default='edit')
    parser.add_argument('--name', action='append', default=[])
    parser.add_argument('--area', action='append', default=[])
    parser.add_argument('--skip', action='append', default=[])
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    if args.action == 'selftest':
        assert select(['search_missing_1mb_file'], [], []) == ['search_missing_1mb_file']
        assert len(select([], ['search'], [])) == 2
        for a in [(['bad'], [], []), ([], ['bad'], []), ([], [], ['bad']), (['search_missing_1mb_file'], [], ['search_missing_1mb_file'])]:
            try:
                select(*a)
            except ValueError:
                continue
            raise AssertionError(a)
        print('selection checks passed')
        return
    if args.action == 'list':
        for name, area in WORKLOADS.items():
            print(f'change {area} {name}')
        print('edit harness selection selftest')
        return
    try:
        selected = select(args.name, args.area, args.skip) if args.tier == 'change' else []
        if args.tier == 'edit' and (args.name or args.area or args.skip):
            raise ValueError('edit tier has no workload selectors')
        for tool in ('cargo', 'rustc', 'cpu-guard'):
            if not shutil.which(tool):
                raise ValueError(f'{tool} missing; install it before running {args.tier}')
        if shutil.disk_usage(ROOT).free < 1024**3:
            raise ValueError('less than 1 GiB free; free space before running benchmarks')
    except ValueError as error:
        parser.error(str(error))
    print('host readiness: informational; use perf-gate-ready before and after timed comparisons')
    for name in WORKLOADS:
        if name not in selected:
            print(f'excluded {name}: not selected')
    if args.action == 'check':
        print('preflight passed')
        return
    if args.tier == 'edit':
        command = [sys.executable, str(ROOT / 'tools/bench_tier.py'), 'selftest']
    else:
        command = ['cpu-guard', '--cores', '4', 'cargo', 'bench', '--locked', '--bench', 'bench', '--', '--exact', '--sample-count', '1', '--sample-size', '1', *(f'bench::{name}' for name in selected)]
    print('command:', ' '.join(command), flush=True)
    if args.dry_run:
        return
    output = ROOT / '.work' / 'bench' / datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    output.mkdir(parents=True)
    start = time.monotonic()
    env = os.environ.copy()
    env['CARGO_BUILD_JOBS'] = '4'
    result = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    (output / 'output.log').write_text(result.stdout)
    if result.returncode == 0 and args.tier == 'change' and any(name not in result.stdout for name in selected):
        print('selected workload missing from output', file=sys.stderr)
        result.returncode = 1
    print(result.stdout, end='')
    print(f'exit={result.returncode} duration={time.monotonic()-start:.2f}s output={output}; completion is not a performance verdict')
    if result.returncode:
        sys.exit(result.returncode)


if __name__ == '__main__':
    main()

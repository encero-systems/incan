#!/usr/bin/ruby
# Research instrumentation: capture exact inputs, phase clocks and kernel footprint, without changing native flags.
require 'json'
require 'open3'
require 'fileutils'
require 'zlib'

collector_started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
base = ENV.fetch('SCALING_OUTPUT')
driver = ENV.fetch('SCALING_DRIVER')
prefix = File.join(base, 'commands', "#{ENV.fetch('SCALING_TRIAL', 'prepare')}-#{Process.pid}")
FileUtils.mkdir_p(File.dirname(prefix))
record = {argv: ARGV, driver: driver, cwd: Dir.pwd, trial: ENV['SCALING_TRIAL'], epoch: Time.now.to_f}
record[:role] = ARGV[0] == '--resident' ? 'daemon' : 'compiler'
record[:environment] = ENV.select do |key, _|
  ['CARGO_MAKEFLAGS', 'RAYON_NUM_THREADS', 'INCAN_HOME', 'INCAN_STDLIB', 'SCALING_DEBUG_INFO'].include?(key)
end
record[:driver_sha256] = ENV.fetch('SCALING_DRIVER_SHA256')
if ARGV[0] == '--unit'
  FileUtils.cp(ARGV[1], prefix + '.unit.json')
end
File.write(prefix + '.json', JSON.pretty_generate(record))
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
environment = {'INCAN_TRACE_DIR'=>ARGV[0] == '--unit' ? prefix + '.traces' : nil}
command = [File.join(base, 'footprint'), driver, *ARGV]
if ARGV[0] == '--unit' && ENV['SCALING_DEBUG_INFO']
  command += ['--rustc-arg', "-Cdebuginfo=#{ENV.fetch('SCALING_DEBUG_INFO')}"]
end
record[:effective_command] = command
File.write(prefix + '.json', JSON.pretty_generate(record))
status = nil
File.open(prefix + '.stdout', 'w') do |out|
  File.open(prefix + '.stderr', 'w') do |err|
    out.sync = err.sync = true
    Open3.popen3(environment, *command, close_others: false) do |input, output, errors, wait|
      input.close
      readers = [[output, out, STDOUT], [errors, err, STDERR]].map do |source, log, destination|
        Thread.new do
          source.each_line do |line|
            log.write("#{Process.clock_gettime(Process::CLOCK_MONOTONIC)-started}\t#{line}")
            destination.write(line)
          end
        end
      end
      readers.each(&:join)
      status = wait.value
    end
  end
end
record[:wall] = Process.clock_gettime(Process::CLOCK_MONOTONIC)-started
record[:status] = status.exitstatus || 128+status.termsig
record[:driver_completed_epoch] = Time.now.to_f
File.write(prefix + '.json', JSON.pretty_generate(record))
postprocess_started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
Dir.glob(prefix+'.traces/*.jsonl').each do |path|
  Zlib::GzipWriter.open(path+'.gz') { |gzip| File.open(path) { |input| IO.copy_stream(input, gzip) } }
  File.unlink(path)
end
record[:postprocess_wall] = Process.clock_gettime(Process::CLOCK_MONOTONIC)-postprocess_started
record[:collector_wall] = Process.clock_gettime(Process::CLOCK_MONOTONIC)-collector_started
File.write(prefix + '.json', JSON.pretty_generate(record))
exit record[:status]

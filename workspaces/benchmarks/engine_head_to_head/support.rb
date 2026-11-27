# Shared clocks, child observation, host samples and evidence records for the engine head-to-head runner.
require 'digest'
require 'fileutils'
require 'json'
require 'open3'

module HeadToHead
  ENGINES = %w[legacy records].freeze
  TRACE_FLAGS = %w[INCAN_ENGINE_TRACE INCAN_NATIVE_TIMING INCAN_NATIVE_PLAN_REPORT].freeze
  # Process roles that keep a home's engine state alive between ordinary commands.
  WORKER_ROLES = %w[--internal-owner --internal-engine --internal-engine-settlement --resident].freeze
  # Inline at most this many bytes of a child's stdout or stderr; larger streams go to a file beside the samples.
  INLINE_BYTES = 4096

  # A required workload did not complete as specified. The observation that failed travels with it.
  class Refusal < StandardError
    attr_reader :evidence

    def initialize(message, evidence = nil)
      super(message)
      @evidence = evidence
    end
  end

  def self.refuse(message, evidence = nil)
    raise Refusal.new(message, evidence)
  end

  def self.require_fact(condition, message, evidence = nil)
    refuse(message, evidence) unless condition
  end

  def self.clock_ns
    Process.clock_gettime(Process::CLOCK_MONOTONIC_RAW, :nanosecond)
  end

  def self.utc
    output, status = Open3.capture2('/bin/date', '-u', '+%FT%TZ')
    refuse('UTC clock read failed') unless status.success?
    output.strip
  end

  def self.sha256(path)
    Digest::SHA256.file(path).hexdigest
  end

  def self.record(path, value)
    File.write(path, JSON.pretty_generate(value) + "\n")
  end

  def self.read_json(path)
    JSON.parse(File.read(path))
  end

  # Clone a tree with APFS copy-on-write; the clone shares storage blocks, so it is not a cold OS page cache.
  def self.clone_tree(source, destination)
    FileUtils.mkdir_p(File.dirname(destination))
    _, errors, status = Open3.capture3('/bin/cp', '-cRp', source, destination)
    refuse("cannot clone #{source} to #{destination}: #{errors}") unless status.success?
    destination
  end

  STORE_PARTS = %w[payloads receipts native-shared verified-payloads snapshots segments].freeze

  # Entries under each record directory of a Bakery: growth between two observations shows new results.
  def self.store_counts(bakery)
    STORE_PARTS.to_h { |part| [part, Dir.glob('**/*', base: File.join(bakery, part)).size] }
  end

  def self.make_directory(path)
    FileUtils.mkdir_p(path, mode: 0o700)
    path
  end

  # Load averages and the par.rb heavy slots held at this instant; slots are omitted when no slot root is given.
  def self.host_sample(slot_root)
    load, status = Open3.capture2('/usr/sbin/sysctl', '-n', 'vm.loadavg')
    refuse('host load unavailable') unless status.success?
    pressure, = Open3.capture2('/usr/sbin/sysctl', '-n', 'kern.memorystatus_vm_pressure_level')
    sample = { load: load.delete('{}').split.map { |value| Float(value) }, memory_pressure: Integer(pressure.strip) }
    sample[:heavy_slots] = held_slots(slot_root) if slot_root
    sample
  end

  def self.held_slots(root)
    (0...7).select do |slot|
      File.open(File.join(root, slot.to_s), File::RDONLY) { |lease| !lease.flock(File::LOCK_EX | File::LOCK_NB) }
    rescue Errno::ENOENT
      false
    end
  end

  # A stream is kept inline when small and valid UTF-8; otherwise it is written beside the samples and named by digest.
  def self.stream_entry(bytes, spill, name)
    entry = { bytes: bytes.bytesize, sha256: Digest::SHA256.hexdigest(bytes) }
    text = bytes.dup.force_encoding(Encoding::UTF_8)
    if bytes.bytesize <= INLINE_BYTES && text.valid_encoding?
      entry[:text] = text
    else
      path = File.join(spill, "#{entry[:sha256]}.#{name}")
      File.binwrite(path, bytes) unless File.exist?(path)
      entry[:file] = path
    end
    entry
  end

  # Run one child to its end. Timed observations read the monotonic clock around spawn, first stdout byte and exit.
  def self.observe(directory, environment, argv, timed:, slot_root:, input: nil)
    return untimed_child(directory, environment, argv, input) unless timed

    before = host_sample(slot_root)
    started = clock_ns
    stdout, stderr, first, status = streamed_child(directory, environment, argv, input)
    ended = clock_ns
    { argv: argv, cwd: directory, environment: environment, stdout: stdout, stderr: stderr,
      exit: status.exitstatus, signal: status.termsig, wall_ms: (ended - started) / 1_000_000.0,
      first_stdout_ms: first && (first - started) / 1_000_000.0, host_before: before, host_after: host_sample(slot_root) }
  end

  def self.untimed_child(directory, environment, argv, input)
    stdout, stderr, status = Open3.capture3(environment, *argv, chdir: directory, binmode: true, stdin_data: input || '')
    { argv: argv, cwd: directory, environment: environment, stdout: stdout, stderr: stderr,
      exit: status.exitstatus, signal: status.termsig }
  end

  def self.streamed_child(directory, environment, argv, input)
    stdout = String.new(encoding: Encoding::BINARY)
    stderr = String.new(encoding: Encoding::BINARY)
    first = nil
    status = Open3.popen3(environment, *argv, chdir: directory) do |stdin, out, err, waiter|
      stdin.write(input) if input
      stdin.close
      readers = [out, err]
      until readers.empty?
        ready, = IO.select(readers)
        ready.each do |reader|
          bytes = reader.read_nonblock(65_536, exception: false)
          next if bytes == :wait_readable

          if bytes.nil?
            readers.delete(reader)
          elsif reader == out
            first ||= clock_ns
            stdout << bytes
          else
            stderr << bytes
          end
        end
      end
      waiter.value
    end
    [stdout, stderr, first, status]
  end

  # Worker, owner and resident processes started from any of `homes`: the ones that keep warm engine state.
  def self.home_workers(homes)
    listing, status = Open3.capture2({ 'LC_ALL' => 'C' }, '/bin/ps', '-Aww', '-o', 'pid=,stat=,command=')
    refuse('process observation failed') unless status.success?
    rows = listing.each_line.map { |line| line.strip.split(/\s+/, 3) }
    rows.select do |_, state, command|
      next false if command.nil? || state.start_with?('Z')

      executable, role = command.split(/\s+/, 3)
      homes.any? { |home| executable.start_with?(home + '/') } && WORKER_ROLES.include?(role)
    end.map { |pid, state, command| { pid: Integer(pid), state: state, command: command } }
  end

  # Stop a home's workers so the next sample starts with its service and workers stopped; report what was stopped.
  def self.quiesce(homes)
    stopped = []
    %w[TERM KILL].each do |signal|
      live = home_workers(homes)
      break if live.empty?

      live.each do |row|
        Process.kill(signal, row.fetch(:pid))
        stopped << row.merge(signal: signal)
      rescue Errno::ESRCH
        nil
      end
      sleep 1
    end
    remaining = home_workers(homes)
    refuse('home workers survived quiescence', remaining) unless remaining.empty?
    stopped
  end

  # Replace each absolute root with a stable placeholder so replies from different arm directories compare exactly.
  def self.normalize(text, roots)
    roots.sort_by { |root, _| -root.length }.reduce(text) { |current, (root, name)| current.gsub(root, name) }
  end

  def self.median(values)
    ordered = values.sort
    middle = ordered.size / 2
    ordered.size.odd? ? ordered[middle] : (ordered[middle - 1] + ordered[middle]) / 2.0
  end

  # Nearest-rank percentile; callers report it only when the sample count supports it.
  def self.percentile(values, fraction)
    ordered = values.sort
    ordered[[(fraction * ordered.size).ceil - 1, 0].max]
  end
end

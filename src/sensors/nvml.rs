use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::struct_wrappers::device::ProcessUtilizationSample;
use nvml_wrapper::{cuda_driver_version_major, cuda_driver_version_minor, Nvml, Device};
use pci_info::PciInfo;
use std::error::Error;
use crate::sensors::{Record, RecordStorage};
use crate::sensors::current_system_time_since_epoch;
use crate::sensors::units::Unit;
use std::fmt;

#[derive(Debug)]
pub struct NvidiaNVMLGpu {
    pub index: usize,
    pub vendor: String,
    pub model: String,
    pub arch: String,
    /// Measurements of power usage, stored as Record instances
    pub record_storage: RecordStorage,
    /// Outer vector stores history, inner vector stores samples for each process
    pub proc_utils: Vec<Vec<ProcessUtilizationSample>>,
}

impl NvidiaNVMLGpu {
    pub fn new(nvml: &Nvml, index: u32) -> Result<NvidiaNVMLGpu, Box<dyn Error>> {
        let device = nvml.device_by_index(index)?;
        let model = device.name()?.to_string();
        let arch = device.architecture()?.to_string();

        Ok(NvidiaNVMLGpu {
            index: index as usize,
            vendor: "nvidia".to_string(),
            model,
            arch,
            record_storage: RecordStorage::new(0),
            proc_utils: Vec::new(),
        })
    }

    /// Returns a new owned Vector being a clone of the current record_buffer.
    /// This does not affect the current buffer but is costly.
    pub fn get_records_passive(&self) -> Vec<Record> {
        let mut result = vec![];
        for r in &self.record_storage.records {
            result.push(Record::new(
                r.timestamp,
                r.value.clone(),
                Unit::MicroWatt,
            ));
        }
        result
    }

    pub fn get_pid_last_util(&self, pid: u32) -> Option<&ProcessUtilizationSample> {
        let last_proc_utils = self.proc_utils.last()?;
        for proc_util in last_proc_utils {
            if proc_util.pid == pid {
                return Some(proc_util);
            }
        }
        None
    }

    pub fn push_proc_utils(&mut self, proc_utils: Vec<ProcessUtilizationSample>) {
        self.proc_utils.push(proc_utils);
    }
}

impl fmt::Display for NvidiaNVMLGpu {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "GPU (index: {}, name: {})", self.index, self.model)
    }
}

#[derive(Debug)]
pub struct NvidiaNVML {
    pub nvml_obj: Nvml,
    pub gpus: Vec<NvidiaNVMLGpu>,
}

impl NvidiaNVML {
    pub fn new() -> Option<NvidiaNVML> {
        // Detect GPU before NVML initialization
        let info = PciInfo::enumerate_pci().unwrap();
        let mut nvidia = false;

        for r in info {
            match r {
                Ok(device) => {
                    if device.vendor_id() == 0x10DE {
                        nvidia = true;
                        break;
                    }
                },
                Err(error) => {}
            }
        }

        if nvidia {
            info!("Nvidia GPU found!");
        } else {
            info!("No Nvidia GPU found!");
            return None;
        }

        // NVML initialization
        let nvml = match Nvml::init() {
            Ok(nvml) => nvml,
            Err(e) => {
                error!("Failed to initialize NVML. Error: {}", e);
                return None;
            }
        };

        let gpus_count = match nvml.device_count() {
            Ok(count) => count,
            Err(e) => {
                error!("Failed to search Nvidia GPUs using NVML. Error: {}", e);
                return None;
            }
        };

        info!("NVML initialized. Found {} Nvidia GPUs!", gpus_count);

        let mut gpus = Vec::with_capacity(gpus_count as usize);
        for i in 0..gpus_count {
            match NvidiaNVMLGpu::new(&nvml, i) {
                Ok(gpu) => gpus.push(gpu),
                Err(e) => {
                    error!("Failed to initialize GPU handler. Index: {}. Error: {}", i, e);
                }
            }
        }

        Some(NvidiaNVML {
            nvml_obj: nvml,
            gpus: gpus,
        })
    }

    pub fn get_gpu_str(&self, index: usize) -> String {
        for gpu in &self.gpus {
            if gpu.index == index {
                return gpu.to_string();
            }
        }
        return format!("GPU (index: {}, error: not found)", index);
    }

    pub fn get_gpus_count(&self) -> usize {
        self.gpus.len()
    }

    fn get_device(&self, index: usize) -> Result<Device, Box<dyn Error>> {
        match self.nvml_obj.device_by_index(index as u32) {
            Err(e) => Err(Box::new(e)),
            Ok(device) => Ok(device),
        }
    }

    pub fn get_gpu_consumption(&self, index: usize) -> Result<Record, Box<dyn Error>> {
        let device = self.get_device(index)?;
        let power_usage_milliwatts = device.power_usage()?;
        // power_usage returns value in milli. We need micro.
        let power_usage_microwatts = (power_usage_milliwatts * 1000).to_string();
        Ok(Record::new(
            current_system_time_since_epoch(),
            power_usage_microwatts,
            Unit::MicroWatt,
        ))
    }

    pub fn refresh_process_utilization(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        let device = self.get_device(index)?;
        let proc_utils = device.process_utilization_stats(None)?;
        self.gpus[index].push_proc_utils(proc_utils);
        Ok(())
    }

    pub fn refresh_records(&mut self) {
        for i in 0..self.get_gpus_count() {
            match self.get_gpu_consumption(i) {
                Ok(record) => self.gpus[i].record_storage.add_record(&record),
                Err(e) => warn!("Failed to refresh record for {}. Error: {}", self.get_gpu_str(i), e),
            }

            match self.refresh_process_utilization(i) {
                Ok(_) => {
                    info!(
                        "Refreshing records for GPU. Count of records: {}",
                        self.gpus[i].record_storage.records.len()
                    );
                },
                Err(e) => {
                    if matches!(e.downcast_ref::<NvmlError>(), Some(NvmlError::NotFound)) {
                        info!("No process found on {}.", self.get_gpu_str(i));
                    } else {
                        warn!("Failed to refresh process utilization for {}. Error: {}", self.get_gpu_str(i), e);
                    }
                }
            }
        }
    }
}

impl Clone for NvidiaNVML {
    fn clone(&self) -> NvidiaNVML {
        // TODO dangerous
        NvidiaNVML::new().unwrap()
    }
}

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
    /// Returns new NvidiaNVMLGpu object if the GPU specified by the index is supported by the NVML, and doesn't have enabled MIG mode.
    pub fn new(nvml: &Nvml, index: u32) -> Result<NvidiaNVMLGpu, Box<dyn Error>> {
        let device = nvml.device_by_index(index)?;
        let model = device.name()?.to_string();
        let arch = device.architecture()?.to_string();

        // Check MIG (Multi-Instance GPU) support. Reading power usage is not available for MIG GPUs.
        // MIG enabled       > problem
        // MIG not enabled   > ok
        // MIG not supported > ok
        // MIG unknown error > hope that reading works
        let mig_obstacle = match device.mig_mode() {
            Ok(mig_mode) => {
                if mig_mode.current > 0 {
                    error!("MIG mode is enabled for GPU {}, name: {}. Power consumption reading doesn't work for cards in MIG mode.", index, model);
                    true
                } else {
                    false
                }
            },
            Err(e) => match e {
                NvmlError::NotSupported => false,
                _ => {
                    error!("Failed to determine MIG mode for GPU {}, name: {}. Error: {}. Let's assume that power consumption reading works.", index, model, e);
                    false
                }
            }
        };
        if mig_obstacle {
            return Err("Power consumption reading doesn't work for cards in MIG mode.".into());
        }

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

    /// Returns last GPU usage record for the process specified by the PID.
    pub fn get_pid_last_util(&self, pid: u32) -> Option<&ProcessUtilizationSample> {
        let last_proc_utils = self.proc_utils.last()?;
        for proc_util in last_proc_utils {
            if proc_util.pid == pid {
                return Some(proc_util);
            }
        }
        None
    }

    /// Adds new processes utilization records.
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
    /// Returns new NvidiaNVML object if at least one Nvidia PCIe device is found and NVML lib initialization is successfull.
    /// Tries to initialize GPU handlers for all supported GPUs. If a problem occurs, returns None.
    pub fn new() -> Option<NvidiaNVML> {
        // Detect Nvidia PCIe device before NVML initialization
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

        // Detect Nvidia GPUs using NVML
        let gpus_count = match nvml.device_count() {
            Ok(count) => count,
            Err(e) => {
                error!("Failed to search Nvidia GPUs using NVML. Error: {}", e);
                return None;
            }
        };

        info!("NVML initialized. Found {} Nvidia GPUs!", gpus_count);

        // Try to init GPU handler for all GPUs found using NVML
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

    /// Returns string with GPU index and name.
    pub fn get_gpu_str(&self, index: usize) -> String {
        for gpu in &self.gpus {
            if gpu.index == index {
                return gpu.to_string();
            }
        }
        return format!("GPU (index: {}, error: not found)", index);
    }

    /// Returns count of the successfully initialized and managed GPUs.
    pub fn get_gpus_count(&self) -> usize {
        self.gpus.len()
    }

    /// Returns NVML Device with specified index.
    fn get_device(&self, index: usize) -> Result<Device, Box<dyn Error>> {
        match self.nvml_obj.device_by_index(index as u32) {
            Err(e) => Err(Box::new(e)),
            Ok(device) => Ok(device),
        }
    }

    /// Returns Record with total power consumption of the GPU specified with index.
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

    /// Check process utilization for the GPU specified with index, and save it the proc_utils structure.
    pub fn refresh_process_utilization(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        let device = self.get_device(index)?;
        let proc_utils = device.process_utilization_stats(None)?;
        self.gpus[index].push_proc_utils(proc_utils);
        Ok(())
    }

    /// Check GPU power consumption and process utilization of all GPUs, and save the values.
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
